//! The dispatch table is also the source for help and status-bar hints.
use super::{
    prompt::Edit,
    state::{Focus, Overlay},
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayCtx {
    None,
    Help,
    Inbox,
    Confirm,
    Picker,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeymapCtx {
    pub overlay: OverlayCtx,
    pub focus: Focus,
}
impl KeymapCtx {
    pub fn new(overlay: Option<&Overlay>, focus: Focus) -> Self {
        Self {
            overlay: match overlay {
                None => OverlayCtx::None,
                Some(Overlay::Help) => OverlayCtx::Help,
                Some(Overlay::Inbox) => OverlayCtx::Inbox,
                Some(Overlay::Confirm(_)) => OverlayCtx::Confirm,
                Some(Overlay::Picker(_)) => OverlayCtx::Picker,
            },
            focus,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    NextFocus,
    PrevFocus,
    Up,
    Down,
    PageUp,
    PageDown,
    HalfUp,
    HalfDown,
    First,
    Last,
    FocusPrompt,
    Thinking,
    Tools,
    Scope,
    Refresh,
    Inbox,
    Help,
    Quit,
    Send,
    Newline,
    Mode,
    Edit(Edit),
    Escape,
    Interrupt,
    Eof,
    Abort,
    Models,
    ThinkingLevels,
    Compact,
    Confirm,
    Cancel,
    Pick,
}
impl Action {
    pub fn movement(self) -> bool {
        matches!(
            self,
            Self::Up
                | Self::Down
                | Self::PageUp
                | Self::PageDown
                | Self::HalfUp
                | Self::HalfDown
                | Self::First
                | Self::Last
                | Self::Edit(
                    Edit::Left
                        | Edit::Right
                        | Edit::Home
                        | Edit::End
                        | Edit::HistoryUp
                        | Edit::HistoryDown
                )
        )
    }
}
#[derive(Clone, Copy, Debug)]
pub struct KeyBinding {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
    pub label: &'static str,
}
impl KeyBinding {
    pub fn matches(self, key: KeyEvent) -> bool {
        let mut code = key.code;
        let mut modifiers = key.modifiers;
        // Legacy terminals can deliver Ctrl-J as LF rather than a chord.
        if code == KeyCode::Char('\n') && modifiers.is_empty() {
            code = KeyCode::Char('j');
            modifiers = KeyModifiers::CONTROL;
        }
        // Return may arrive as its C0 spelling, either a Ctrl-M chord or bare CR.
        if (code == KeyCode::Char('m') && modifiers == KeyModifiers::CONTROL)
            || (code == KeyCode::Char('\r') && modifiers.is_empty())
        {
            code = KeyCode::Enter;
            modifiers = KeyModifiers::NONE;
        }
        if matches!(code,KeyCode::Char(c) if c.is_uppercase() || matches!(c,'?' )) {
            modifiers.remove(KeyModifiers::SHIFT);
        }
        if code == KeyCode::BackTab {
            modifiers.remove(KeyModifiers::SHIFT);
        }
        code == self.code && modifiers == self.modifiers
    }
    pub fn event(self) -> KeyEvent {
        KeyEvent::new(self.code, self.modifiers)
    }
}
type Binding = (KeyBinding, Action, &'static str);
#[derive(Clone, Copy, PartialEq, Eq)]
enum Where {
    All,
    Base,
    Panes,
    Sessions,
    Transcript,
    Prompt,
    Overlay,
    Confirm,
    Picker,
}
const fn key(code: KeyCode, modifiers: KeyModifiers, label: &'static str) -> KeyBinding {
    KeyBinding {
        code,
        modifiers,
        label,
    }
}
use Action as A;
use KeyCode::*;
use KeyModifiers as M;
// Each binding appears once here; context projections below preserve this order.
const TABLE: &[(Where, Binding)] = &[
    (
        Where::All,
        (
            key(Char('c'), M::CONTROL, "Ctrl-C"),
            A::Interrupt,
            "clear prompt / quit",
        ),
    ),
    (
        Where::All,
        (
            key(Char('x'), M::CONTROL, "Ctrl-X"),
            A::Abort,
            "stop running turn",
        ),
    ),
    (Where::All, (key(F(1), M::NONE, "F1"), A::Help, "help")),
    (
        Where::Base,
        (key(Tab, M::NONE, "Tab"), A::NextFocus, "next focus"),
    ),
    (
        Where::Base,
        (
            key(BackTab, M::NONE, "Shift-Tab"),
            A::PrevFocus,
            "previous focus",
        ),
    ),
    (
        Where::Base,
        (
            key(Tab, M::SHIFT, "Shift-Tab"),
            A::PrevFocus,
            "previous focus",
        ),
    ),
    (
        Where::Panes,
        (key(Char('t'), M::NONE, "t"), A::Thinking, "toggle thinking"),
    ),
    (
        Where::Panes,
        (key(Char('o'), M::NONE, "o"), A::Tools, "cycle tool detail"),
    ),
    (
        Where::Panes,
        (
            key(Char('a'), M::NONE, "a"),
            A::Scope,
            "project / all projects",
        ),
    ),
    (
        Where::Panes,
        (key(Char('r'), M::NONE, "r"), A::Refresh, "refresh"),
    ),
    (
        Where::Panes,
        (key(Char('i'), M::NONE, "i"), A::Inbox, "peek inbox"),
    ),
    (
        Where::Panes,
        (key(Char('?'), M::NONE, "?"), A::Help, "help"),
    ),
    (
        Where::Panes,
        (key(Char('q'), M::NONE, "q"), A::Quit, "quit"),
    ),
    (
        Where::Panes,
        (key(Char('m'), M::NONE, "m"), A::Models, "choose model"),
    ),
    (
        Where::Panes,
        (
            key(Char('T'), M::NONE, "T"),
            A::ThinkingLevels,
            "choose thinking level",
        ),
    ),
    (
        Where::Panes,
        (key(Char('C'), M::NONE, "C"), A::Compact, "compact session"),
    ),
    (Where::Panes, (key(Up, M::NONE, "↑"), A::Up, "up")),
    (Where::Panes, (key(Char('k'), M::NONE, "k"), A::Up, "up")),
    (Where::Panes, (key(Down, M::NONE, "↓"), A::Down, "down")),
    (
        Where::Panes,
        (key(Char('j'), M::NONE, "j"), A::Down, "down"),
    ),
    (
        Where::Panes,
        (key(PageUp, M::NONE, "PgUp"), A::PageUp, "page up"),
    ),
    (
        Where::Panes,
        (key(PageDown, M::NONE, "PgDn"), A::PageDown, "page down"),
    ),
    (
        Where::Panes,
        (key(Home, M::NONE, "Home"), A::First, "first / top"),
    ),
    (
        Where::Panes,
        (key(Char('g'), M::NONE, "g"), A::First, "first / top"),
    ),
    (
        Where::Panes,
        (key(End, M::NONE, "End"), A::Last, "last / follow"),
    ),
    (
        Where::Panes,
        (key(Char('G'), M::NONE, "G"), A::Last, "last / follow"),
    ),
    (
        Where::Sessions,
        (key(Enter, M::NONE, "Enter"), A::FocusPrompt, "focus prompt"),
    ),
    (
        Where::Transcript,
        (
            key(Char('u'), M::CONTROL, "Ctrl-U"),
            A::HalfUp,
            "half page up",
        ),
    ),
    (
        Where::Transcript,
        (
            key(Char('d'), M::CONTROL, "Ctrl-D"),
            A::HalfDown,
            "half page down",
        ),
    ),
    (
        Where::Prompt,
        (key(Enter, M::NONE, "Enter"), A::Send, "send"),
    ),
    (
        Where::Prompt,
        (key(Enter, M::ALT, "Alt-Enter"), A::Newline, "new line"),
    ),
    (
        Where::Prompt,
        (
            key(Char('j'), M::CONTROL, "Ctrl-J / LF"),
            A::Newline,
            "new line",
        ),
    ),
    (
        Where::Prompt,
        (
            key(Char('t'), M::CONTROL, "Ctrl-T"),
            A::Mode,
            "auto / steer / follow-up",
        ),
    ),
    (
        Where::Prompt,
        (key(Left, M::NONE, "←"), A::Edit(Edit::Left), "cursor left"),
    ),
    (
        Where::Prompt,
        (
            key(Right, M::NONE, "→"),
            A::Edit(Edit::Right),
            "cursor right",
        ),
    ),
    (
        Where::Prompt,
        (
            key(Home, M::NONE, "Home"),
            A::Edit(Edit::Home),
            "line start",
        ),
    ),
    (
        Where::Prompt,
        (key(End, M::NONE, "End"), A::Edit(Edit::End), "line end"),
    ),
    (
        Where::Prompt,
        (
            key(Char('a'), M::CONTROL, "Ctrl-A"),
            A::Edit(Edit::Home),
            "line start",
        ),
    ),
    (
        Where::Prompt,
        (
            key(Char('e'), M::CONTROL, "Ctrl-E"),
            A::Edit(Edit::End),
            "line end",
        ),
    ),
    (
        Where::Prompt,
        (
            key(Backspace, M::NONE, "Backspace"),
            A::Edit(Edit::Backspace),
            "delete before",
        ),
    ),
    (
        Where::Prompt,
        (
            key(Delete, M::NONE, "Delete"),
            A::Edit(Edit::Delete),
            "delete after",
        ),
    ),
    (
        Where::Prompt,
        (
            key(Char('w'), M::CONTROL, "Ctrl-W"),
            A::Edit(Edit::WordBefore),
            "delete word before",
        ),
    ),
    (
        Where::Prompt,
        (
            key(Char('u'), M::CONTROL, "Ctrl-U"),
            A::Edit(Edit::LineBefore),
            "delete to line start",
        ),
    ),
    (
        Where::Prompt,
        (
            key(Char('k'), M::CONTROL, "Ctrl-K"),
            A::Edit(Edit::LineAfter),
            "delete to line end",
        ),
    ),
    (
        Where::Prompt,
        (
            key(Up, M::NONE, "↑"),
            A::Edit(Edit::HistoryUp),
            "previous send",
        ),
    ),
    (
        Where::Prompt,
        (
            key(Down, M::NONE, "↓"),
            A::Edit(Edit::HistoryDown),
            "next send",
        ),
    ),
    (
        Where::Prompt,
        (
            key(Esc, M::NONE, "Esc"),
            A::Escape,
            "dismiss error / transcript",
        ),
    ),
    (
        Where::Prompt,
        (
            key(Char('d'), M::CONTROL, "Ctrl-D"),
            A::Eof,
            "quit empty prompt",
        ),
    ),
    (
        Where::Overlay,
        (key(Esc, M::NONE, "Esc"), A::Escape, "close overlay"),
    ),
    (
        Where::Confirm,
        (key(Char('y'), M::NONE, "y"), A::Confirm, "confirm"),
    ),
    (
        Where::Confirm,
        (key(Char('n'), M::NONE, "n"), A::Cancel, "cancel"),
    ),
    (
        Where::Picker,
        (key(Enter, M::NONE, "Enter"), A::Pick, "choose"),
    ),
];
/// Help uses the same bindings and context groups as dispatch, including overlay-local keys.
pub(crate) fn help_groups() -> Vec<(&'static str, Vec<Binding>)> {
    [
        ("anywhere", Where::All),
        ("outside overlays", Where::Base),
        ("panes", Where::Panes),
        ("sessions", Where::Sessions),
        ("transcript", Where::Transcript),
        ("prompt", Where::Prompt),
        ("overlays", Where::Overlay),
        ("confirm", Where::Confirm),
        ("picker", Where::Picker),
    ]
    .into_iter()
    .map(|(title, context)| {
        (
            title,
            TABLE
                .iter()
                .filter(|(where_, _)| *where_ == context)
                .map(|(_, binding)| *binding)
                .collect(),
        )
    })
    .collect()
}
fn applies(w: Where, c: KeymapCtx) -> bool {
    match w {
        Where::All => true,
        Where::Base => c.overlay == OverlayCtx::None,
        Where::Panes => c.overlay == OverlayCtx::None && c.focus != Focus::Prompt,
        Where::Sessions => c.overlay == OverlayCtx::None && c.focus == Focus::Sessions,
        Where::Transcript => c.overlay == OverlayCtx::None && c.focus == Focus::Transcript,
        Where::Prompt => c.overlay == OverlayCtx::None && c.focus == Focus::Prompt,
        Where::Overlay => c.overlay != OverlayCtx::None,
        Where::Confirm => c.overlay == OverlayCtx::Confirm,
        Where::Picker => c.overlay == OverlayCtx::Picker,
    }
}
pub fn bindings(ctx: KeymapCtx) -> &'static [Binding] {
    // Build immutable context projections once, from the single table above.
    static CACHE: std::sync::OnceLock<Vec<Vec<Binding>>> = std::sync::OnceLock::new();
    let focus = match ctx.focus {
        Focus::Sessions => 0,
        Focus::Transcript => 1,
        Focus::Prompt => 2,
    };
    let overlay = match ctx.overlay {
        OverlayCtx::None => 0,
        OverlayCtx::Help => 1,
        OverlayCtx::Inbox => 2,
        OverlayCtx::Confirm => 3,
        OverlayCtx::Picker => 4,
    };
    let cache = CACHE.get_or_init(|| {
        [
            OverlayCtx::None,
            OverlayCtx::Help,
            OverlayCtx::Inbox,
            OverlayCtx::Confirm,
            OverlayCtx::Picker,
        ]
        .into_iter()
        .flat_map(|overlay| {
            [Focus::Sessions, Focus::Transcript, Focus::Prompt]
                .into_iter()
                .map(move |focus| {
                    TABLE
                        .iter()
                        .filter(|(w, _)| applies(*w, KeymapCtx { overlay, focus }))
                        .map(|(_, b)| *b)
                        .collect()
                })
        })
        .collect()
    });
    &cache[overlay * 3 + focus]
}
pub fn action(ctx: KeymapCtx, key: KeyEvent) -> Option<Action> {
    if key.kind == KeyEventKind::Release {
        return None;
    }
    bindings(ctx)
        .iter()
        .find(|(binding, _, _)| binding.matches(key))
        .map(|(_, a, _)| *a)
        .filter(|a| key.kind != KeyEventKind::Repeat || a.movement())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ctrl_m_and_cr_match_return_in_every_context() {
        for overlay in [
            OverlayCtx::None,
            OverlayCtx::Help,
            OverlayCtx::Inbox,
            OverlayCtx::Confirm,
            OverlayCtx::Picker,
        ] {
            for focus in [Focus::Sessions, Focus::Transcript, Focus::Prompt] {
                let ctx = KeymapCtx { overlay, focus };
                for kind in [
                    KeyEventKind::Press,
                    KeyEventKind::Repeat,
                    KeyEventKind::Release,
                ] {
                    let mut enter = KeyEvent::new(Enter, M::NONE);
                    enter.kind = kind;
                    for mut compatible in [
                        KeyEvent::new(Char('m'), M::CONTROL),
                        KeyEvent::new(Char('\r'), M::NONE),
                    ] {
                        compatible.kind = kind;
                        assert_eq!(action(ctx, compatible), action(ctx, enter), "{ctx:?}");
                    }
                }
            }
        }
        let ctx = KeymapCtx {
            overlay: OverlayCtx::None,
            focus: Focus::Prompt,
        };
        assert_eq!(
            action(ctx, KeyEvent::new(Char('m'), M::CONTROL)),
            Some(A::Send)
        );
        let ctx = KeymapCtx {
            focus: Focus::Sessions,
            ..ctx
        };
        assert_eq!(
            action(ctx, KeyEvent::new(Char('m'), M::CONTROL)),
            Some(A::FocusPrompt)
        );
    }
    #[test]
    fn newline_compatibility_and_kind_filtering() {
        let ctx = KeymapCtx {
            overlay: OverlayCtx::None,
            focus: Focus::Prompt,
        };
        for key in [
            KeyEvent::new(Char('j'), M::CONTROL),
            KeyEvent::new(Char('\n'), M::NONE),
            KeyEvent::new(Enter, M::ALT),
        ] {
            assert_eq!(action(ctx, key), Some(A::Newline));
        }
        assert_eq!(action(ctx, KeyEvent::new(Char('j'), M::NONE)), None);
        let mut key = KeyEvent::new(Enter, M::NONE);
        key.kind = KeyEventKind::Repeat;
        assert_eq!(action(ctx, key), None);
        key.kind = KeyEventKind::Release;
        assert_eq!(action(ctx, key), None);
        key = KeyEvent::new(Left, M::NONE);
        key.kind = KeyEventKind::Repeat;
        assert_eq!(action(ctx, key), Some(A::Edit(Edit::Left)));
    }
    #[test]
    fn table_has_labels_no_conflicts_and_overlay_contexts_capture_pane_keys() {
        for overlay in [
            OverlayCtx::None,
            OverlayCtx::Help,
            OverlayCtx::Inbox,
            OverlayCtx::Confirm,
            OverlayCtx::Picker,
        ] {
            for focus in [Focus::Sessions, Focus::Transcript, Focus::Prompt] {
                let ctx = KeymapCtx { overlay, focus };
                let table = bindings(ctx);
                for (i, (binding, expected, label)) in table.iter().enumerate() {
                    assert!(!binding.label.is_empty() && !label.is_empty());
                    assert_eq!(action(ctx, binding.event()), Some(*expected));
                    assert!(
                        !table[..i]
                            .iter()
                            .any(|(other, _, _)| other.matches(binding.event())),
                        "duplicate {}",
                        binding.label
                    );
                }
                if overlay != OverlayCtx::None {
                    assert_eq!(action(ctx, KeyEvent::new(Char('q'), M::NONE)), None);
                    assert_eq!(action(ctx, KeyEvent::new(Tab, M::NONE)), None);
                }
            }
        }
    }
    #[test]
    fn terminal_shift_encodings_are_compatible() {
        let ctx = KeymapCtx {
            overlay: OverlayCtx::None,
            focus: Focus::Sessions,
        };
        for key in [
            KeyEvent::new(BackTab, M::NONE),
            KeyEvent::new(BackTab, M::SHIFT),
            KeyEvent::new(Tab, M::SHIFT),
        ] {
            assert_eq!(action(ctx, key), Some(A::PrevFocus));
        }
        assert_eq!(
            action(ctx, KeyEvent::new(Char('G'), M::SHIFT)),
            Some(A::Last)
        );
        assert_eq!(
            action(ctx, KeyEvent::new(Char('T'), M::SHIFT)),
            Some(A::ThinkingLevels)
        );
        assert_eq!(
            action(ctx, KeyEvent::new(Char('?'), M::SHIFT)),
            Some(A::Help)
        );
    }
}
