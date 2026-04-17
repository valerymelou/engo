//! `engo init` — create an `engo.toml` in the working directory.
//!
//! Two modes:
//! * **Interactive** (default): use auto-detection to seed defaults, then
//!   prompt the user to confirm each value with `dialoguer`.
//! * **Non-interactive** (`--yes` or when stdin is not a TTY, or when any
//!   required flag is passed explicitly): skip prompts, use supplied flags
//!   and detected defaults. This makes the command scriptable and testable.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::Args as ClapArgs;
use dialoguer::{theme::ColorfulTheme, Input, Select};
use engo_core::config::{
    AiConfig, AiProvider, Config, LanguagesConfig, ProjectConfig, ProjectFormat,
    DEFAULT_CONFIG_FILENAME,
};

/// `engo init` command arguments.
#[derive(Debug, Clone, ClapArgs)]
pub struct Args {
    /// Directory to initialize (defaults to the current directory).
    #[arg(long, default_value = ".")]
    pub dir: PathBuf,

    /// Skip prompts — accept detected defaults and the supplied flags.
    #[arg(long, short)]
    pub yes: bool,

    /// Overwrite an existing engo.toml.
    #[arg(long)]
    pub force: bool,

    /// Source language BCP-47 tag (e.g. `en`, `en-US`).
    #[arg(long)]
    pub source: Option<String>,

    /// Comma-separated target languages (e.g. `fr,de,es`).
    #[arg(long, value_delimiter = ',')]
    pub targets: Vec<String>,

    /// `xliff`, `arb`, or `json`. Auto-detected if omitted.
    #[arg(long)]
    pub format: Option<ProjectFormatArg>,

    /// `anthropic`, `openai`, or `engo-cloud`.
    #[arg(long, default_value = "anthropic")]
    pub provider: AiProviderArg,

    /// Override the default model name.
    #[arg(long)]
    pub model: Option<String>,

    /// Glob for translation files (defaults to the auto-detected suggestion).
    #[arg(long)]
    pub files_glob: Option<String>,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum ProjectFormatArg {
    Xliff,
    Arb,
    Json,
}

impl From<ProjectFormatArg> for ProjectFormat {
    fn from(v: ProjectFormatArg) -> Self {
        match v {
            ProjectFormatArg::Xliff => ProjectFormat::Xliff,
            ProjectFormatArg::Arb => ProjectFormat::Arb,
            ProjectFormatArg::Json => ProjectFormat::Json,
        }
    }
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum AiProviderArg {
    Anthropic,
    Openai,
    #[value(name = "engo-cloud")]
    EngoCloud,
}

impl From<AiProviderArg> for AiProvider {
    fn from(v: AiProviderArg) -> Self {
        match v {
            AiProviderArg::Anthropic => AiProvider::Anthropic,
            AiProviderArg::Openai => AiProvider::Openai,
            AiProviderArg::EngoCloud => AiProvider::EngoCloud,
        }
    }
}

pub fn run(args: Args) -> Result<()> {
    let dir = args
        .dir
        .canonicalize()
        .with_context(|| format!("cannot resolve directory {}", args.dir.display()))?;
    let config_path = dir.join(DEFAULT_CONFIG_FILENAME);

    if config_path.exists() && !args.force {
        bail!(
            "{} already exists. Pass --force to overwrite.",
            config_path.display()
        );
    }

    let detection = engo_core::detect(&dir);
    if let Some(d) = &detection {
        eprintln!("detected: {} ({})", d.format.as_str(), d.reason);
    } else {
        eprintln!("no project type detected — you'll pick one manually.");
    }

    let non_interactive = args.yes || !is_stdin_tty();

    let format: ProjectFormat = match args.format {
        Some(f) => f.into(),
        None => match (&detection, non_interactive) {
            (Some(d), _) => d.format,
            (None, true) => bail!("no project type detected; pass --format xliff|arb|json"),
            (None, false) => prompt_format()?,
        },
    };

    let default_glob = detection
        .as_ref()
        .map(|d| d.suggested_glob.clone())
        .unwrap_or_else(|| default_glob_for(format).to_string());

    let files_glob = match args.files_glob {
        Some(g) => g,
        None if non_interactive => default_glob,
        None => Input::<String>::with_theme(&ColorfulTheme::default())
            .with_prompt("Glob for translation files")
            .default(default_glob)
            .interact_text()?,
    };

    let source = match args.source {
        Some(s) => s,
        None if non_interactive => "en".to_string(),
        None => Input::<String>::with_theme(&ColorfulTheme::default())
            .with_prompt("Source language (BCP-47)")
            .default("en".to_string())
            .interact_text()?,
    };

    let targets: Vec<String> = if !args.targets.is_empty() {
        args.targets.iter().map(|s| s.trim().to_string()).collect()
    } else if non_interactive {
        bail!("--targets is required in non-interactive mode (e.g. --targets fr,de)");
    } else {
        let raw: String = Input::with_theme(&ColorfulTheme::default())
            .with_prompt("Target languages (comma-separated)")
            .default("fr,de".to_string())
            .interact_text()?;
        raw.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    };

    let provider: AiProvider = args.provider.into();
    let model = args
        .model
        .unwrap_or_else(|| default_model_for(provider).to_string());

    let cfg = Config {
        project: ProjectConfig {
            format,
            files_glob,
            description: None,
        },
        languages: LanguagesConfig { source, targets },
        ai: AiConfig {
            provider,
            model,
            batch_size: 15,
            endpoint: None,
        },
        glossary: BTreeMap::new(),
    };

    cfg.save(&config_path)
        .with_context(|| format!("writing {}", config_path.display()))?;

    eprintln!("wrote {}", config_path.display());
    eprintln!("next: set ANTHROPIC_API_KEY (or OPENAI_API_KEY) and run `engo translate --list`.");
    Ok(())
}

fn prompt_format() -> Result<ProjectFormat> {
    let choices = ["xliff", "arb", "json"];
    let idx = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Project format")
        .items(&choices)
        .default(0)
        .interact()?;
    Ok(match idx {
        0 => ProjectFormat::Xliff,
        1 => ProjectFormat::Arb,
        _ => ProjectFormat::Json,
    })
}

fn default_glob_for(fmt: ProjectFormat) -> &'static str {
    match fmt {
        ProjectFormat::Xliff => "locales/*.xlf",
        ProjectFormat::Arb => "lib/l10n/*.arb",
        ProjectFormat::Json => "locales/*.json",
    }
}

fn default_model_for(provider: AiProvider) -> &'static str {
    match provider {
        AiProvider::Anthropic => "claude-haiku-4-5",
        AiProvider::Openai => "gpt-4o-mini",
        AiProvider::EngoCloud => "claude-haiku-4-5",
    }
}

fn is_stdin_tty() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal()
}
