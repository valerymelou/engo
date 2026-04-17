//! Parsers and writers for the i18n file formats Engo supports.
//!
//! Phase 1 ships XLIFF 1.2 and 2.0. Phase 3 adds ARB (Flutter) and plain
//! nested JSON (i18next, next-intl, vue-i18n).

pub mod arb;
pub mod json;
pub mod xliff;

/// Semantic translation state, normalized across XLIFF 1.2 and 2.0.
///
/// XLIFF 1.2 carries this on `<target state="...">`. XLIFF 2.0 carries it on
/// `<segment state="...">`. We collapse both dialects' vocabularies into these
/// four buckets so the diff engine doesn't have to care about the version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnitState {
    /// Target missing, empty, or explicitly marked as needing translation.
    /// `needs-translation`, `new`, `needs-adaptation`, `needs-l10n`,
    /// `needs-review-translation` (1.2) and `initial` (2.0).
    NeedsTranslation,
    /// Translated but not yet reviewed/approved. `translated` in both dialects.
    Translated,
    /// Reviewed / signed-off / final. Engo never overwrites a final unit.
    /// `final` and `signed-off` (1.2), `reviewed` and `final` (2.0).
    Final,
    /// A state we don't know about. Treated conservatively as "do not touch".
    Other,
}

impl UnitState {
    /// Whether a translation pass should *try* to fill or replace this unit.
    ///
    /// `Final` and `Other` are never touched by a normal run — `--force` can
    /// still override this decision explicitly at the CLI layer.
    pub fn should_translate(self) -> bool {
        matches!(self, UnitState::NeedsTranslation)
    }
}
