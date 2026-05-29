//! Turbo Vision facade — the ONLY module in the crate that may `use turbo_vision`.
//!
//! Boundary rule (spec §8 / §14): every other module talks to the TUI
//! exclusively through plain domain types. No `turbo_vision` type may leak past
//! this file. Keeping the dependency confined here makes the backend swappable
//! and keeps the rest of the crate testable without a terminal.
//!
//! Tty boundary (spec §11): anything that constructs an `Application`, dialog,
//! menu bar, status line, outline, or message box requires a real terminal and
//! is NOT unit-tested. The logic those wrappers consume lives below the facade
//! in pure, tested functions. (Those wrappers arrive in later tasks.)

use turbo_vision::prelude::*;

/// Compile-time proof that the crate links against Turbo Vision.
///
/// Real views land in later tasks; this keeps the dependency genuinely
/// exercised at link time and gives Task 1 a tty-free thing to assert on.
pub fn tv_available() -> bool {
    // `Rect` construction needs no terminal, so referencing it here proves the
    // crate is linked without requiring a tty.
    let _ = Rect::new(0, 0, 1, 1);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facade_boundary_compiles() {
        assert!(tv_available());
    }
}
