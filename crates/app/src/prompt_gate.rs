#[derive(Default)]
pub struct PromptGate {
    open: usize,
}

impl PromptGate {
    pub fn opened(&mut self) {
        self.open += 1;
    }

    pub fn answered(&mut self) {
        self.open = self.open.saturating_sub(1);
    }

    pub fn blocks_input(&self) -> bool {
        self.open > 0
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
        let mut gate = PromptGate::default();
        gate.opened();
        gate.opened();
        gate.answered();
        assert!(gate.blocks_input());
        gate.answered();
        assert!(!gate.blocks_input());
    }

    #[test]
    fn answering_without_a_prompt_keeps_the_gate_open_for_the_next_one() {
        let mut gate = PromptGate::default();
        gate.answered();
        gate.opened();
        assert!(gate.blocks_input());
    }
}
