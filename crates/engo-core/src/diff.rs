//! State-aware diff: given a parsed XLIFF view, return the units that still
//! need translation (or *all* units when `force` is set).
//!
//! This module is intentionally dumb: it does not read files, call the AI, or
//! write anything. The CLI orchestrates I/O and drives this function per file.

use crate::formats::xliff::{TransUnit, XliffView};
use crate::formats::UnitState;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DiffOptions {
    /// Re-translate units even if already in `Translated` state. `Final` is
    /// *still* never touched — user intent on reviewed content is sacred.
    pub force: bool,
}

/// Which units in `view` need to be sent to the translator.
pub fn pending(view: &XliffView, opts: DiffOptions) -> Vec<&TransUnit> {
    view.units
        .iter()
        .filter(|u| should_translate(u, opts))
        .collect()
}

/// Same decision as [`pending`], but returns indices into `view.units` so the
/// caller can mutate in place (used by the CLI to batch under a semaphore).
pub fn pending_indices(view: &XliffView, opts: DiffOptions) -> Vec<usize> {
    view.units
        .iter()
        .enumerate()
        .filter(|(_, u)| should_translate(u, opts))
        .map(|(i, _)| i)
        .collect()
}

fn should_translate(u: &TransUnit, opts: DiffOptions) -> bool {
    match u.state {
        UnitState::Final => false,
        UnitState::Other => false,
        UnitState::NeedsTranslation => true,
        UnitState::Translated => opts.force,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::xliff::{parse, XliffVersion};

    const FIXTURE: &[u8] = include_bytes!("../tests/fixtures/simple-1.2.xlf");

    #[test]
    fn default_diff_returns_only_needs_translation() {
        let view = parse(FIXTURE).unwrap();
        assert_eq!(view.version, XliffVersion::V1_2);
        let pending = pending(&view, DiffOptions::default());
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, "greeting");
    }

    #[test]
    fn force_returns_translated_but_never_final() {
        let view = parse(FIXTURE).unwrap();
        let pending = pending(&view, DiffOptions { force: true });
        let ids: Vec<&str> = pending.iter().map(|u| u.id.as_str()).collect();
        assert!(ids.contains(&"greeting"));
        assert!(ids.contains(&"login_button"));
        assert!(
            !ids.contains(&"copyright"),
            "final units must never be retranslated"
        );
    }
}
