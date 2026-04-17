//! `engo translate` — the main loop.
//!
//! Broad shape:
//!
//! 1. Load `engo.toml` and expand `project.files_glob` to a list of paths.
//! 2. Hand the paths to [`engo_core::plan_jobs`] which dispatches by format
//!    (XLIFF, ARB, JSON) and returns a [`TranslationJob`] per target file.
//! 3. `--list` → print the pending table and stop.
//! 4. Otherwise: probe the cache, batch the misses, fan out under a semaphore,
//!    validate placeholder signatures, patch, write atomically with `.bak`.
//!
//! The cache lives at `.engo/cache.db` next to the config. It's safe to
//! delete; worst case is one extra round-trip per cached pair.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use clap::Args as ClapArgs;
use engo_ai::{AnthropicProvider, TranslationRequest, Translator};
use engo_core::cache::{self, Cache, CacheKey};
use engo_core::catalog::{self, PendingUnit, TranslationJob};
use engo_core::config::{AiProvider, Config, DEFAULT_CONFIG_FILENAME};
use engo_core::diff::DiffOptions;
use engo_core::safety::{self, CleanStatus};
use engo_core::validate;
use futures::stream::{FuturesUnordered, StreamExt};
use indicatif::{ProgressBar, ProgressStyle};
use tokio::sync::Semaphore;

#[derive(Debug, Clone, ClapArgs)]
pub struct Args {
    /// Path to the engo.toml (defaults to ./engo.toml).
    #[arg(long, default_value = "engo.toml")]
    pub config: PathBuf,

    /// Only report pending units — don't call the AI and don't write files.
    #[arg(long)]
    pub list: bool,

    /// Compute translations and show them, but don't write to disk.
    #[arg(long)]
    pub dry_run: bool,

    /// Also re-translate units already in `translated` state (never `final`).
    #[arg(long)]
    pub force: bool,

    /// Restrict processing to a single target language tag.
    #[arg(long)]
    pub target: Option<String>,

    /// Maximum concurrent provider calls. Defaults to 4.
    #[arg(long, default_value_t = 4)]
    pub concurrency: usize,

    /// Skip the git-clean check. Writes still leave a `.bak` next to each
    /// overwritten file so recovery is possible.
    #[arg(long)]
    pub allow_dirty: bool,

    /// Skip the translation cache (still writes to it).
    #[arg(long)]
    pub no_cache: bool,
}

pub async fn run(args: Args) -> Result<()> {
    let cfg_path = resolve_config_path(&args.config)?;
    let cfg = Config::load(&cfg_path)
        .with_context(|| format!("loading {}", cfg_path.display()))?;
    let cfg_dir = cfg_path
        .parent()
        .ok_or_else(|| anyhow!("config path has no parent"))?
        .to_path_buf();

    let files = expand_files(&cfg_dir, &cfg.project.files_glob)?;
    if files.is_empty() {
        eprintln!(
            "no files matched glob {:?} under {}",
            cfg.project.files_glob,
            cfg_dir.display()
        );
        return Ok(());
    }

    let opts = DiffOptions { force: args.force };
    let mut jobs = catalog::plan_jobs(&cfg, &files, opts)
        .with_context(|| "planning translation jobs")?;

    if let Some(filter) = args.target.as_deref() {
        jobs.retain(|j| j.target_lang == filter);
    }

    if args.list {
        print_list(&jobs);
        return Ok(());
    }

    if jobs.iter().all(|j| j.pending.is_empty()) {
        eprintln!("nothing to translate — all targets are up to date.");
        return Ok(());
    }

    // Safety: refuse to clobber user work unless they've opted in with
    // `--allow-dirty` (and we'll still write a `.bak` in that case).
    if !args.dry_run && !args.allow_dirty {
        enforce_clean_repo(&cfg_dir)?;
    }

    let translator: Arc<dyn Translator> = build_translator(&cfg)?;
    let semaphore = Arc::new(Semaphore::new(args.concurrency.max(1)));

    let cache_path = cfg_dir.join(".engo").join("cache.db");
    let cache = if args.no_cache {
        None
    } else {
        match Cache::open(&cache_path) {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!(
                    "warning: cache unavailable at {}: {e}; continuing without cache",
                    cache_path.display()
                );
                None
            }
        }
    };
    let glossary_version = cache::glossary_version(&cfg.glossary);

    for job in jobs {
        process_job(
            job,
            &cfg,
            args.dry_run,
            translator.clone(),
            semaphore.clone(),
            cache.as_ref(),
            &glossary_version,
        )
        .await?;
    }

    Ok(())
}

fn resolve_config_path(arg: &Path) -> Result<PathBuf> {
    let p = if arg.is_absolute() {
        arg.to_path_buf()
    } else {
        std::env::current_dir()?.join(arg)
    };
    if p.is_dir() {
        Ok(p.join(DEFAULT_CONFIG_FILENAME))
    } else {
        Ok(p)
    }
}

fn expand_files(cfg_dir: &Path, pattern: &str) -> Result<Vec<PathBuf>> {
    let full = cfg_dir.join(pattern);
    let full_str = full.to_string_lossy();
    let mut out = Vec::new();
    for entry in glob::glob(&full_str).with_context(|| format!("bad glob {:?}", pattern))? {
        match entry {
            Ok(p) if p.is_file() => out.push(p),
            Ok(_) => {}
            Err(e) => eprintln!("glob error: {e}"),
        }
    }
    out.sort();
    Ok(out)
}

fn enforce_clean_repo(dir: &Path) -> Result<()> {
    match safety::repo_clean(dir) {
        CleanStatus::Clean | CleanStatus::NotAGitRepo => Ok(()),
        CleanStatus::Dirty(_) => bail!(
            "working tree is not clean — commit/stash first or rerun with --allow-dirty \
             (we'll still write .bak files next to each overwritten file)"
        ),
        CleanStatus::Unknown(e) => {
            // Don't block progress if `git` is unavailable; just warn.
            eprintln!("warning: git status check failed ({e}); continuing");
            Ok(())
        }
    }
}

fn print_list(jobs: &[TranslationJob]) {
    let mut total_pending = 0usize;
    for job in jobs {
        let rel = job.target_path.display();
        println!(
            "{rel}  ({} → {})  pending: {}",
            job.source_lang,
            job.target_lang,
            job.pending.len()
        );
        if job.pending.is_empty() {
            continue;
        }
        total_pending += job.pending.len();
        let max_id = job.pending.iter().map(|u| u.id.len()).max().unwrap_or(0);
        for u in &job.pending {
            println!(
                "  {:<width$}  {}",
                u.id,
                short(&u.source, 80),
                width = max_id
            );
        }
        println!();
    }
    println!("{total_pending} total pending unit(s)");
}

fn short(s: &str, max: usize) -> String {
    let trimmed = s.trim().replace('\n', " ");
    if trimmed.chars().count() <= max {
        trimmed
    } else {
        let mut out: String = trimmed.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn build_translator(cfg: &Config) -> Result<Arc<dyn Translator>> {
    match cfg.ai.provider {
        AiProvider::Anthropic => {
            let p = AnthropicProvider::from_env(cfg.ai.model.clone())
                .map_err(|e| anyhow!("anthropic provider: {e}"))?;
            Ok(Arc::new(p))
        }
        AiProvider::Openai => {
            bail!("OpenAI provider is not implemented yet (coming in a later phase)")
        }
        AiProvider::EngoCloud => {
            bail!("Engo Cloud provider is not implemented yet (coming in a later phase)")
        }
    }
}

async fn process_job(
    job: TranslationJob,
    cfg: &Config,
    dry_run: bool,
    translator: Arc<dyn Translator>,
    semaphore: Arc<Semaphore>,
    cache: Option<&Cache>,
    glossary_version: &str,
) -> Result<()> {
    if job.pending.is_empty() {
        return Ok(());
    }

    let source_lang = job.source_lang.clone();
    let target_lang = job.target_lang.clone();
    let model = cfg.ai.model.clone();
    let app_description = cfg.project.description.clone();
    let glossary = cfg.glossary.clone();
    let batch_size = cfg.ai.batch_size.max(1);

    // 1. Probe cache for each pending unit. Hits go straight into `accepted`;
    //    misses become provider requests.
    let source_by_id: HashMap<String, String> = job
        .pending
        .iter()
        .map(|u| (u.id.clone(), u.source.clone()))
        .collect();

    let mut accepted: HashMap<String, String> = HashMap::new();
    let mut rejected: Vec<(String, String)> = Vec::new();
    let mut requests: Vec<TranslationRequest> = Vec::new();

    if let Some(c) = cache {
        for u in &job.pending {
            let key = CacheKey {
                source: &u.source,
                source_lang: &source_lang,
                target_lang: &target_lang,
                context: u.context.as_deref(),
                model: &model,
                glossary_version,
            };
            match c.get(&key) {
                Ok(Some(cached)) => {
                    accepted.insert(u.id.clone(), cached);
                }
                Ok(None) => requests.push(pending_to_request(u)),
                Err(e) => {
                    tracing::warn!("cache read failed: {e}; falling back to provider");
                    requests.push(pending_to_request(u));
                }
            }
        }
    } else {
        requests.extend(job.pending.iter().map(pending_to_request));
    }

    let cache_hits = accepted.len();

    eprintln!(
        "{}  ({} → {})  pending: {}  cache hits: {}  calling AI on: {}",
        job.target_path.display(),
        source_lang,
        target_lang,
        job.pending.len(),
        cache_hits,
        requests.len(),
    );

    // 2. Batch provider calls under a semaphore. Progress bar tracks batches.
    if !requests.is_empty() {
        let batches: Vec<Vec<TranslationRequest>> = requests
            .chunks(batch_size)
            .map(|c| c.to_vec())
            .collect();

        let pb = ProgressBar::new(batches.len() as u64);
        pb.set_style(
            ProgressStyle::with_template(
                "  [{bar:30}] {pos}/{len} batches  {elapsed_precise}",
            )
            .unwrap()
            .progress_chars("=> "),
        );

        let mut futs = FuturesUnordered::new();
        for batch in batches {
            let sem = semaphore.clone();
            let translator = translator.clone();
            let src = source_lang.clone();
            let tgt = target_lang.clone();
            let desc = app_description.clone();
            let gloss = glossary.clone();

            futs.push(async move {
                let permit = sem.acquire_owned().await.expect("semaphore closed");
                let out = translator
                    .translate_batch(&src, &tgt, desc.as_deref(), &gloss, &batch)
                    .await;
                drop(permit);
                (batch, out)
            });
        }

        while let Some((batch, result)) = futs.next().await {
            match result {
                Ok(responses) => {
                    let got_ids: std::collections::HashSet<_> =
                        responses.iter().map(|r| r.id.clone()).collect();
                    for r in responses {
                        if let Some(src) = source_by_id.get(&r.id) {
                            match validate::validate_pair(src, &r.target) {
                                Ok(()) => {
                                    // Write-through to cache on acceptance.
                                    if let Some(c) = cache {
                                        let ctx = job
                                            .pending
                                            .iter()
                                            .find(|u| u.id == r.id)
                                            .and_then(|u| u.context.clone());
                                        let key = CacheKey {
                                            source: src,
                                            source_lang: &source_lang,
                                            target_lang: &target_lang,
                                            context: ctx.as_deref(),
                                            model: &model,
                                            glossary_version,
                                        };
                                        if let Err(e) = c.put(&key, &r.target) {
                                            tracing::warn!("cache write failed: {e}");
                                        }
                                    }
                                    accepted.insert(r.id, r.target);
                                }
                                Err(e) => rejected.push((r.id, e.to_string())),
                            }
                        } else {
                            rejected.push((
                                r.id,
                                "model returned an id we didn't ask for".to_string(),
                            ));
                        }
                    }
                    for req in &batch {
                        if !got_ids.contains(&req.id) {
                            rejected.push((
                                req.id.clone(),
                                "model dropped this id".to_string(),
                            ));
                        }
                    }
                }
                Err(e) => {
                    for req in batch {
                        rejected.push((req.id, format!("provider error: {e}")));
                    }
                }
            }
            pb.inc(1);
        }
        pb.finish_and_clear();
    }

    if !rejected.is_empty() {
        eprintln!("  ! {} unit(s) failed validation:", rejected.len());
        for (id, reason) in &rejected {
            eprintln!("    - {id}: {reason}");
        }
    }

    if dry_run {
        if accepted.is_empty() {
            eprintln!("  (dry-run) nothing accepted to preview");
            return Ok(());
        }
        eprintln!("  (dry-run) {} translation(s) pending:", accepted.len());
        let mut ids: Vec<_> = accepted.keys().cloned().collect();
        ids.sort();
        for id in ids {
            let src = source_by_id.get(&id).cloned().unwrap_or_default();
            let tgt = &accepted[&id];
            eprintln!("    + {id}");
            eprintln!("      - {}", short(&src, 120));
            eprintln!("      + {}", short(tgt, 120));
        }
        return Ok(());
    }

    if accepted.is_empty() {
        eprintln!("  nothing to write.");
        return Ok(());
    }

    let new_bytes = catalog::apply(&job, &accepted)
        .with_context(|| format!("patching {}", job.target_path.display()))?;
    safety::atomic_write_with_backup(&job.target_path, &new_bytes)
        .with_context(|| format!("writing {}", job.target_path.display()))?;
    eprintln!(
        "  wrote {} translation(s) to {}",
        accepted.len(),
        job.target_path.display()
    );
    Ok(())
}

fn pending_to_request(u: &PendingUnit) -> TranslationRequest {
    TranslationRequest {
        id: u.id.clone(),
        source: u.source.clone(),
        context: u.context.clone(),
    }
}
