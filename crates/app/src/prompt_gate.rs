use std::cell::Cell;
use std::rc::Rc;

#[derive(Default)]
pub struct PromptGate {
    open: Rc<Cell<usize>>,
}

// Dropping the guard reopens the gate, so a prompt task that is cancelled cannot leave the
// workspace blocked.
pub struct PromptGuard {
    open: Rc<Cell<usize>>,
}

impl PromptGate {
    pub fn open(&self) -> PromptGuard {
        self.open.set(self.open.get() + 1);
        PromptGuard {
            open: self.open.clone(),
        }
    }

    pub fn blocks_input(&self) -> bool {
        self.open.get() > 0
    }
}

impl Drop for PromptGuard {
    fn drop(&mut self) {
        self.open.set(self.open.get().saturating_sub(1));
    }
}

#[cfg(test)]
mod tests {
    use super::PromptGate;

    #[test]
    fn a_new_gate_lets_input_through() {
        assert!(!PromptGate::default().blocks_input());
    }

    #[test]
    fn input_is_blocked_until_every_open_prompt_is_answered() {
        let gate = PromptGate::default();
        let first = gate.open();
        let second = gate.open();
        drop(first);
        assert!(gate.blocks_input());
        drop(second);
        assert!(!gate.blocks_input());
    }

    #[test]
    fn a_dropped_guard_reopens_the_gate() {
        let gate = PromptGate::default();
        {
            let _guard = gate.open();
            assert!(gate.blocks_input());
        }
        assert!(!gate.blocks_input());
    }
}
