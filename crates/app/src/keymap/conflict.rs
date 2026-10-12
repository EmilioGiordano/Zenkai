use super::chord::Chord;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Workspace,
    SettingsWindow,
    Both,
}

impl Scope {
    pub fn of(context: Option<&str>) -> Scope {
        match context {
            None => Scope::Both,
            Some(context) if context.contains("SettingsWindow") => Scope::SettingsWindow,
            Some(_) => Scope::Workspace,
        }
    }

    pub fn overlaps(self, other: Scope) -> bool {
        self == Scope::Both || other == Scope::Both || self == other
    }
}

// One command holding one chord in one window. `editable` means the user can take the chord
// away from its owner; the rest (cell navigation, dialog keys) cannot be given up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Claim {
    pub chord: Chord,
    pub owner: String,
    pub scope: Scope,
    pub editable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Conflict {
    Replaceable(Vec<String>),
    Fixed(String),
}

pub fn find(claims: &[Claim], command: &str, scopes: &[Scope], chord: &Chord) -> Option<Conflict> {
    let mut editable: Vec<String> = Vec::new();
    for claim in claims {
        if claim.chord != *chord
            || claim.owner == command
            || !scopes.iter().any(|scope| scope.overlaps(claim.scope))
        {
            continue;
        }
        if !claim.editable {
            return Some(Conflict::Fixed(claim.owner.clone()));
        }
        if !editable.contains(&claim.owner) {
            editable.push(claim.owner.clone());
        }
    }
    (!editable.is_empty()).then_some(Conflict::Replaceable(editable))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(chord: &str, owner: &str, scope: Scope, editable: bool) -> Claim {
        Claim {
            chord: Chord::parse(chord).unwrap(),
            owner: owner.into(),
            scope,
            editable,
        }
    }

    #[test]
    fn the_same_window_overlaps_and_the_other_window_does_not() {
        let claims = [claim("ctrl-b", "Bold", Scope::Workspace, true)];
        let chord = Chord::parse("ctrl-b").unwrap();
        assert_eq!(
            find(&claims, "Sidebar", &[Scope::Workspace], &chord),
            Some(Conflict::Replaceable(vec!["Bold".into()]))
        );
        assert_eq!(
            find(&claims, "Detect", &[Scope::SettingsWindow], &chord),
            None
        );
        assert!(find(&claims, "Detect", &[Scope::Both], &chord).is_some());
    }

    #[test]
    fn a_command_never_conflicts_with_itself() {
        let claims = [claim("ctrl-b", "Bold", Scope::Workspace, true)];
        let chord = Chord::parse("ctrl-b").unwrap();
        assert_eq!(find(&claims, "Bold", &[Scope::Workspace], &chord), None);
    }

    #[test]
    fn a_fixed_claim_wins_over_editable_ones() {
        let claims = [
            claim("ctrl-a", "Copy", Scope::Workspace, true),
            claim("ctrl-a", "SelectAll", Scope::Both, false),
        ];
        let chord = Chord::parse("ctrl-a").unwrap();
        assert_eq!(
            find(&claims, "Bold", &[Scope::Workspace], &chord),
            Some(Conflict::Fixed("SelectAll".into()))
        );
    }

    #[test]
    fn other_chords_do_not_conflict() {
        let claims = [claim("ctrl-b", "Bold", Scope::Workspace, true)];
        let chord = Chord::parse("ctrl-alt-j").unwrap();
        assert_eq!(find(&claims, "Sidebar", &[Scope::Workspace], &chord), None);
    }
}
