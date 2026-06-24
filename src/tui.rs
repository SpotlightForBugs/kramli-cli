use std::cmp::Ordering;
use std::collections::{hash_map::DefaultHasher, BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::io::{self, Cursor};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use image::{load_from_memory, DynamicImage, ImageDecoder, ImageReader};
use ratatui::backend::CrosstermBackend;
use ratatui::prelude::*;
use ratatui::widgets::{
    Block, Borders, Clear, List as TuiList, ListItem as TuiListItem, ListState, Paragraph, Wrap,
};
use ratatui_image::{
    picker::{Picker, ProtocolType},
    protocol::{Protocol, StatefulProtocol},
    FilterType, Image as TuiImage, Resize, StatefulImage,
};
use sentry::{Hub, SentryFutureExt};
use serde_json::{Map, Value};
use tokio::process::Command as TokioCommand;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use crate::api::ApiClient;
use crate::config::Config;
use crate::i18n::{tr, tr_args};
use crate::models::{
    Attachment, ItemComment, ListItem, ListState as ApiListState, Profile, ShoppingList,
};

const ACCENT: Color = Color::Rgb(126, 200, 255);
const STATUS_COLOR: Color = Color::Rgb(126, 231, 155);
const SELECTED_BG: Color = Color::Rgb(35, 103, 197);
const MUTED_TEXT: Color = Color::Reset;
const CANCEL_BG: Color = Color::Rgb(96, 104, 122);
const SAVE_BG: Color = Color::Rgb(38, 132, 78);
const DRAG_TARGET_COLOR: Color = Color::Yellow;
const BOOTSTRAP_ICON_BASE_URL: &str = "https://icons.getbootstrap.com/assets/icons";
const KRAMLI_BOOTSTRAP_ICON_BASE_URL_ENV: &str = "KRAMLI_BOOTSTRAP_ICON_BASE_URL";
const DEFAULT_LIST_ICON: &str = "tag";
const ARCHIVED_LIST_ICON: &str = "archive";
const KRAMLI_DEVICE_LABEL_ENV: &str = "KRAMLI_DEVICE_LABEL";
const KRAMLI_AUTO_HANDOFF_ENV: &str = "KRAMLI_AUTO_HANDOFF";
const KRAMLI_TUI_IMAGE_PROTOCOL_ENV: &str = "KRAMLI_TUI_IMAGE_PROTOCOL";
const KRAMLI_TUI_IMAGES_ENV: &str = "KRAMLI_TUI_IMAGES";
const TERM_ENV: &str = "TERM";
const TERM_PROGRAM_ENV: &str = "TERM_PROGRAM";
const LC_TERMINAL_ENV: &str = "LC_TERMINAL";
const KITTY_WINDOW_ID_ENV: &str = "KITTY_WINDOW_ID";
const ITERM_SESSION_ID_ENV: &str = "ITERM_SESSION_ID";
const WT_SESSION_ENV: &str = "WT_SESSION";
const KRAMLI_ICON_STYLE_ENV: &str = "KRAMLI_ICON_STYLE";
const KRAMLI_TUI_THEME_ENV: &str = "KRAMLI_TUI_THEME";
const COLORFGBG_ENV: &str = "COLORFGBG";
const KRAMLI_TUI_ICON_COLOR_ENV: &str = "KRAMLI_TUI_ICON_COLOR";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ViewMode {
    List,
    Kanban,
    Calendar,
}

impl ViewMode {
    fn next(self) -> Self {
        match self {
            Self::List => Self::Kanban,
            Self::Kanban => Self::Calendar,
            Self::Calendar => Self::List,
        }
    }

    fn prev(self) -> Self {
        match self {
            Self::List => Self::Calendar,
            Self::Kanban => Self::List,
            Self::Calendar => Self::Kanban,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FocusPane {
    Lists,
    Items,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NavigationAction {
    NextMode,
    PreviousMode,
    SwitchMode(ViewMode),
    MoveMonth(i32),
    MoveHorizontal { delta: i64, fallback: FocusPane },
    MoveSelection(isize),
    Enter,
    Escape,
    Help,
    EdgeItem(bool),
    Ignore,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MouseButtons {
    left_down: bool,
    left_drag: bool,
    left_up: bool,
}

impl MouseButtons {
    fn from_kind(kind: MouseEventKind) -> Option<Self> {
        match kind {
            MouseEventKind::Down(MouseButton::Left) => Some(Self {
                left_down: true,
                left_drag: false,
                left_up: false,
            }),
            MouseEventKind::Drag(MouseButton::Left) => Some(Self {
                left_down: false,
                left_drag: true,
                left_up: false,
            }),
            MouseEventKind::Up(MouseButton::Left) => Some(Self {
                left_down: false,
                left_drag: false,
                left_up: true,
            }),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FooterAction {
    Add,
    Refresh,
    Filter,
    Edit,
    ToggleDone,
    Delete,
    OpenImage,
    Comment,
    Undo,
    Members,
    Invite,
    Help,
    Quit,
}

impl FooterAction {
    fn chip_shortcut(self) -> &'static str {
        match self {
            Self::Help => "?",
            Self::Add => "A",
            Self::Edit => "E",
            Self::ToggleDone => "SPC",
            Self::Delete => "D",
            Self::Filter => "/",
            Self::Refresh => "R",
            Self::Comment => "C",
            Self::OpenImage => "O",
            Self::Members => "M",
            Self::Invite => "I",
            Self::Undo => "U",
            Self::Quit => "Q",
        }
    }

    fn chip_label(self) -> String {
        match self {
            Self::Help => tr("tui-footer-help"),
            Self::Add => tr("tui-footer-add"),
            Self::Edit => tr("tui-footer-edit"),
            Self::ToggleDone => tr("tui-footer-done"),
            Self::Delete => tr("tui-footer-delete"),
            Self::Filter => tr("tui-footer-search"),
            Self::Refresh => tr("tui-footer-refresh"),
            Self::Comment => tr("tui-footer-comment"),
            Self::OpenImage => tr("tui-footer-image"),
            Self::Members => tr("tui-footer-members"),
            Self::Invite => tr("tui-footer-invite"),
            Self::Undo => tr("tui-footer-undo"),
            Self::Quit => tr("tui-footer-quit"),
        }
    }

    fn key_env_name(self) -> &'static str {
        match self {
            Self::Add => "KRAMLI_TUI_KEY_ADD",
            Self::Refresh => "KRAMLI_TUI_KEY_REFRESH",
            Self::Filter => "KRAMLI_TUI_KEY_FILTER",
            Self::Edit => "KRAMLI_TUI_KEY_EDIT",
            Self::ToggleDone => "KRAMLI_TUI_KEY_DONE",
            Self::Delete => "KRAMLI_TUI_KEY_DELETE",
            Self::OpenImage => "KRAMLI_TUI_KEY_IMAGE",
            Self::Comment => "KRAMLI_TUI_KEY_COMMENT",
            Self::Undo => "KRAMLI_TUI_KEY_UNDO",
            Self::Members => "KRAMLI_TUI_KEY_MEMBERS",
            Self::Invite => "KRAMLI_TUI_KEY_INVITE",
            Self::Help => "KRAMLI_TUI_KEY_HELP",
            Self::Quit => "KRAMLI_TUI_KEY_QUIT",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct KeyBinding {
    code: KeyCode,
    modifiers: KeyModifiers,
    label: String,
}

impl KeyBinding {
    fn new(code: KeyCode, modifiers: KeyModifiers, label: &str) -> Self {
        Self {
            code,
            modifiers,
            label: label.to_string(),
        }
    }

    fn matches(&self, key: KeyEvent) -> bool {
        let mut key_modifiers = key
            .modifiers
            .intersection(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT);
        let expected_modifiers = self
            .modifiers
            .intersection(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT);
        if !expected_modifiers.contains(KeyModifiers::SHIFT) {
            key_modifiers.remove(KeyModifiers::SHIFT);
        }
        if expected_modifiers != key_modifiers {
            return false;
        }
        key_codes_match(&self.code, &key.code)
    }
}

#[derive(Clone, Debug)]
struct KeyBindings {
    bindings: Vec<(FooterAction, KeyBinding)>,
}

impl KeyBindings {
    fn from_env() -> Self {
        Self::from_sources(|action| std::env::var(action.key_env_name()).ok())
    }

    fn from_sources(mut source: impl FnMut(FooterAction) -> Option<String>) -> Self {
        let mut bindings = default_key_bindings();
        for (action, binding) in &mut bindings {
            let Some(raw) = source(*action) else {
                continue;
            };
            if let Some(parsed) = parse_key_binding(&raw) {
                *binding = parsed;
            }
        }
        Self { bindings }
    }

    fn action_for_key(&self, key: KeyEvent) -> Option<FooterAction> {
        self.bindings
            .iter()
            .find_map(|(action, binding)| binding.matches(key).then_some(*action))
    }

    fn label_for(&self, action: FooterAction) -> &str {
        self.bindings
            .iter()
            .find_map(|(candidate, binding)| {
                (*candidate == action).then_some(binding.label.as_str())
            })
            .unwrap_or_else(|| action.chip_shortcut())
    }
}

fn default_key_bindings() -> Vec<(FooterAction, KeyBinding)> {
    vec![
        (
            FooterAction::Help,
            KeyBinding::new(KeyCode::Char('?'), KeyModifiers::empty(), "?"),
        ),
        (
            FooterAction::Add,
            KeyBinding::new(KeyCode::Char('a'), KeyModifiers::empty(), "A"),
        ),
        (
            FooterAction::Edit,
            KeyBinding::new(KeyCode::Char('e'), KeyModifiers::empty(), "E"),
        ),
        (
            FooterAction::ToggleDone,
            KeyBinding::new(KeyCode::Char(' '), KeyModifiers::empty(), "SPC"),
        ),
        (
            FooterAction::Delete,
            KeyBinding::new(KeyCode::Char('d'), KeyModifiers::empty(), "D"),
        ),
        (
            FooterAction::Filter,
            KeyBinding::new(KeyCode::Char('/'), KeyModifiers::empty(), "/"),
        ),
        (
            FooterAction::Refresh,
            KeyBinding::new(KeyCode::Char('r'), KeyModifiers::empty(), "R"),
        ),
        (
            FooterAction::Comment,
            KeyBinding::new(KeyCode::Char('c'), KeyModifiers::empty(), "C"),
        ),
        (
            FooterAction::OpenImage,
            KeyBinding::new(KeyCode::Char('o'), KeyModifiers::empty(), "O"),
        ),
        (
            FooterAction::Members,
            KeyBinding::new(KeyCode::Char('m'), KeyModifiers::empty(), "M"),
        ),
        (
            FooterAction::Invite,
            KeyBinding::new(KeyCode::Char('i'), KeyModifiers::empty(), "I"),
        ),
        (
            FooterAction::Undo,
            KeyBinding::new(KeyCode::Char('u'), KeyModifiers::empty(), "U"),
        ),
        (
            FooterAction::Quit,
            KeyBinding::new(KeyCode::Char('q'), KeyModifiers::empty(), "Q"),
        ),
    ]
}

fn parse_key_binding(raw: &str) -> Option<KeyBinding> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let (mut modifiers, key) = parse_key_binding_parts(trimmed)?;
    let (mut code, mut label) = key_code_label(&key)?;

    if code == KeyCode::Tab && modifiers.contains(KeyModifiers::SHIFT) {
        code = KeyCode::BackTab;
        modifiers.remove(KeyModifiers::SHIFT);
        label = "S+Tab".to_string();
    }

    let prefix = key_binding_modifier_label(modifiers);
    Some(KeyBinding::new(
        code,
        modifiers,
        &format!("{prefix}{label}"),
    ))
}

fn parse_key_binding_parts(trimmed: &str) -> Option<(KeyModifiers, String)> {
    let normalized = trimmed.to_ascii_lowercase().replace('-', "+");
    let mut modifiers = KeyModifiers::empty();
    let mut key_part: Option<&str> = None;
    for part in normalized
        .split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        match part {
            "ctrl" | "control" => modifiers |= KeyModifiers::CONTROL,
            "alt" | "option" | "meta" => modifiers |= KeyModifiers::ALT,
            "shift" => modifiers |= KeyModifiers::SHIFT,
            key => key_part = Some(key),
        }
    }

    Some((modifiers, key_part?.to_string()))
}

fn key_code_label(key: &str) -> Option<(KeyCode, String)> {
    key_alias_code_label(key)
        .or_else(|| function_key_code_label(key))
        .or_else(|| single_char_key_code_label(key))
}

fn key_alias_code_label(key: &str) -> Option<(KeyCode, String)> {
    let aliases = [
        ("space", KeyCode::Char(' '), "SPC"),
        ("spc", KeyCode::Char(' '), "SPC"),
        ("esc", KeyCode::Esc, "Esc"),
        ("escape", KeyCode::Esc, "Esc"),
        ("enter", KeyCode::Enter, "Enter"),
        ("return", KeyCode::Enter, "Enter"),
        ("tab", KeyCode::Tab, "Tab"),
        ("backtab", KeyCode::BackTab, "S-Tab"),
        ("shift+tab", KeyCode::BackTab, "S-Tab"),
    ];

    aliases
        .iter()
        .find(|(name, _, _)| *name == key)
        .map(|(_, code, label)| (*code, (*label).to_string()))
}

fn function_key_code_label(key: &str) -> Option<(KeyCode, String)> {
    let number = key.strip_prefix('f')?.parse::<u8>().ok()?;
    (1..=12)
        .contains(&number)
        .then(|| (KeyCode::F(number), format!("F{number}")))
}

fn single_char_key_code_label(key: &str) -> Option<(KeyCode, String)> {
    let mut chars = key.chars();
    let ch = chars.next()?;
    chars
        .next()
        .is_none()
        .then(|| (KeyCode::Char(ch), ch.to_ascii_uppercase().to_string()))
}

fn key_binding_modifier_label(modifiers: KeyModifiers) -> String {
    let mut label = String::default();
    if modifiers.contains(KeyModifiers::CONTROL) {
        label.push_str("C+");
    }
    if modifiers.contains(KeyModifiers::ALT) {
        label.push_str("A+");
    }
    if modifiers.contains(KeyModifiers::SHIFT) {
        label.push_str("S+");
    }
    label
}

fn key_codes_match(expected: &KeyCode, actual: &KeyCode) -> bool {
    match (expected, actual) {
        (KeyCode::Char(expected), KeyCode::Char(actual)) => expected.eq_ignore_ascii_case(actual),
        _ => expected == actual,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EditorField {
    Text,
    Quantity,
    DueDate,
    DueTime,
    PlannedDate,
    PlannedTime,
    Reminder,
    ReminderTime,
    ReminderOffsets,
    TravelTimeMinutes,
    Priority,
    Tags,
    Progress,
    Notes,
}

impl EditorField {
    fn label(self) -> String {
        match self {
            Self::Text => tr("label-text"),
            Self::Quantity => tr("label-quantity"),
            Self::DueDate => tr("label-due"),
            Self::DueTime => tr("label-due-time"),
            Self::PlannedDate => tr("label-planned"),
            Self::PlannedTime => tr("label-planned-time"),
            Self::Reminder => tr("label-reminder"),
            Self::ReminderTime => tr("label-reminder-time"),
            Self::ReminderOffsets => tr("label-reminder-offsets"),
            Self::TravelTimeMinutes => tr("label-travel-time"),
            Self::Priority => "!".to_string(),
            Self::Tags => tr("label-tags"),
            Self::Progress => tr("label-state"),
            Self::Notes => tr("label-notes"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EditorMode {
    Create,
    Edit,
    Comment,
    Filter,
}

#[derive(Clone, Debug)]
struct KanbanColumn {
    name: String,
    is_done: bool,
}

#[derive(Clone, Debug)]
struct EditorState {
    mode: EditorMode,
    item_id: Option<i64>,
    text: String,
    quantity: String,
    due_date: String,
    due_time: String,
    planned_date: String,
    planned_time: String,
    reminder: String,
    reminder_time: String,
    reminder_offsets: String,
    travel_time_minutes: String,
    priority: String,
    tags: String,
    progress: String,
    notes: String,
    active_field: EditorField,
}

struct DetailImageState {
    source: String,
    protocol: StatefulProtocol,
}

enum LoadMessage {
    Profile(Result<Profile, String>),
    Lists(Result<Vec<ShoppingList>, String>),
    Items {
        list_id: i64,
        result: Result<Vec<ListItem>, String>,
    },
    Comments {
        item_id: i64,
        result: Result<Vec<ItemComment>, String>,
    },
    DetailImage {
        source: String,
        result: Result<Vec<u8>, String>,
    },
    ProfileImage {
        source: String,
        result: Result<Vec<u8>, String>,
    },
    ListIcon {
        icon: String,
        result: Result<DynamicImage, String>,
    },
    OpenImage {
        source: String,
        result: Result<String, String>,
    },
    AcceptTerms {
        result: Result<Value, String>,
    },
    AutoHandoffDue {
        list_id: i64,
        list_name: String,
    },
    AutoHandoffSent,
}

struct App {
    api: ApiClient,
    tx: UnboundedSender<LoadMessage>,
    rx: UnboundedReceiver<LoadMessage>,
    picker: Picker,
    lists: Vec<ShoppingList>,
    items: Vec<ListItem>,
    items_cache: HashMap<i64, Vec<ListItem>>,
    comments_cache: HashMap<i64, Vec<ItemComment>>,
    selected_list: usize,
    selected_item: usize,
    list_scroll: usize,
    item_scroll: usize,
    item_filter: String,
    mode: ViewMode,
    focus: FocusPane,
    status: Option<String>,
    inline_images_enabled: bool,
    loading_lists: bool,
    loading_items_for: Option<i64>,
    pending_detail_image: Option<String>,
    pending_profile_image: Option<String>,
    pending_open_image: Option<String>,
    pending_list_icons: HashSet<String>,
    failed_list_icons: HashSet<String>,
    last_item_click: Option<(ViewMode, usize, Instant)>,
    kanban_drag_item: Option<usize>,
    kanban_drag_source_column: Option<usize>,
    kanban_drag_target_column: Option<usize>,
    kanban_drag_started: bool,
    calendar_drag_item: Option<usize>,
    calendar_drag_source_date: Option<SimpleDate>,
    calendar_drag_target_date: Option<SimpleDate>,
    calendar_drag_started: bool,
    calendar_selected_date: Option<SimpleDate>,
    calendar_visible_month: Option<SimpleDate>,
    beta_consent_pending: bool,
    legal_consent_pending: bool,
    legal_accepting: bool,
    legal_pending_docs: Vec<String>,
    initial_load_started: bool,
    should_quit: bool,
    show_help: bool,
    editor: Option<EditorState>,
    detail_image: Option<DetailImageState>,
    detail_image_note: Option<String>,
    profile_name: Option<String>,
    profile_photo_url: Option<String>,
    profile_image: Option<DetailImageState>,
    list_icon_images: HashMap<String, Protocol>,
    bootstrap_icons_enabled: bool,
    image_runtime_info: Option<String>,
    image_runtime_debug: Vec<String>,
    key_bindings: KeyBindings,
    pending_auto_handoff_list_id: Option<i64>,
}

#[derive(Clone, Copy, Debug)]
struct UiLayout {
    lists: Rect,
    content: Rect,
    footer: Rect,
    tab_chunks: [Rect; 3],
}

#[derive(Clone, Copy, Debug)]
struct EditorLayout {
    outer: Rect,
    progress: Rect,
    field: Rect,
    hint: Rect,
    prev: Rect,
    next: Rect,
    save: Rect,
    cancel: Rect,
}

#[derive(Clone, Copy, Debug)]
struct BetaConsentLayout {
    outer: Rect,
    body: Rect,
    hint: Rect,
    accept: Rect,
    decline: Rect,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct SimpleDate {
    year: i32,
    month: u32,
    day: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CalendarDateHit {
    rect: Rect,
    date: SimpleDate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CalendarItemHit {
    rect: Rect,
    item_index: usize,
}

#[derive(Debug)]
struct CalendarLayout {
    title: String,
    month_title: String,
    agenda_title: String,
    month_area: Rect,
    agenda_area: Rect,
    month_lines: Vec<Line<'static>>,
    agenda_lines: Vec<Line<'static>>,
    date_hits: Vec<CalendarDateHit>,
    item_hits: Vec<CalendarItemHit>,
}

#[derive(Debug)]
struct ListPanelRow {
    list_index: Option<usize>,
    depth: usize,
    label: String,
}

const ITEM_EDITOR_FIELDS: [EditorField; 14] = [
    EditorField::Text,
    EditorField::Quantity,
    EditorField::DueDate,
    EditorField::DueTime,
    EditorField::PlannedDate,
    EditorField::PlannedTime,
    EditorField::Reminder,
    EditorField::ReminderTime,
    EditorField::ReminderOffsets,
    EditorField::TravelTimeMinutes,
    EditorField::Priority,
    EditorField::Tags,
    EditorField::Progress,
    EditorField::Notes,
];
const SIMPLE_EDITOR_FIELDS: [EditorField; 1] = [EditorField::Text];

/// Run the full-screen terminal UI.
pub(crate) async fn run_tui() -> Result<(), String> {
    let cfg = Config::load();
    let api = ApiClient::new(&cfg)?;
    run_tui_with_terminal_factory(
        api,
        cfg.bootstrap_icons_enabled(),
        init_terminal,
        restore_terminal,
        |_| {},
    )
    .await
}

async fn run_tui_with_terminal_factory<B, Init, Restore, Prepare>(
    api: ApiClient,
    bootstrap_icons_enabled: bool,
    init_terminal_fn: Init,
    restore_terminal_fn: Restore,
    prepare_app: Prepare,
) -> Result<(), String>
where
    B: Backend,
    Init: FnOnce() -> Result<Terminal<B>, String>,
    Restore: FnOnce(&mut Terminal<B>) -> Result<(), String>,
    Prepare: FnOnce(&mut App),
{
    let mut terminal = init_terminal_fn()?;
    let mut terminal_guard = TerminalCleanupGuard::new();
    let outcome = run_tui_session_internal(
        &mut terminal,
        api,
        bootstrap_icons_enabled,
        restore_terminal_fn,
        prepare_app,
    )
    .await;
    if outcome.restore_succeeded {
        terminal_guard.dismiss();
    }
    outcome.result
}

struct TuiSessionOutcome {
    result: Result<(), String>,
    restore_succeeded: bool,
}

async fn run_tui_session_internal<B, Restore, Prepare>(
    terminal: &mut Terminal<B>,
    api: ApiClient,
    bootstrap_icons_enabled: bool,
    restore_terminal_fn: Restore,
    prepare_app: Prepare,
) -> TuiSessionOutcome
where
    B: Backend,
    Restore: FnOnce(&mut Terminal<B>) -> Result<(), String>,
    Prepare: FnOnce(&mut App),
{
    let transaction = crate::telemetry::TraceTransaction::start("tui.session", "ui");
    transaction.set_tag("mode", "interactive");
    let mut app = App::new(api, bootstrap_icons_enabled);
    prepare_app(&mut app);
    let init_draw = terminal
        .draw(|frame| draw_ui(frame, &mut app))
        .map_err(|e| e.to_string());

    let session_result = match init_draw {
        Ok(_) => {
            let image_pref = image_protocol_preference();
            if image_pref.shows_inline_images() {
                app.status = Some(format!("{}...", tr("label-image")));
                if let Err(error) = terminal.draw(|frame| draw_ui(frame, &mut app)) {
                    Err(error.to_string())
                } else {
                    let (picker, inline_images_enabled, image_runtime_info, image_runtime_debug) =
                        build_image_picker(image_pref);
                    app.set_picker(picker);
                    app.set_inline_images_enabled(inline_images_enabled);
                    if cfg!(debug_assertions) {
                        app.image_runtime_info = Some(image_runtime_info);
                        app.image_runtime_debug = image_runtime_debug;
                    }
                    if !app.initial_load_started {
                        app.initial_load_started = true;
                        app.start_initial_load();
                    }
                    app.status = Some(tr("cli-interactive-beta-notice"));
                    run_event_loop(terminal, &mut app).await
                }
            } else {
                let (picker, inline_images_enabled, image_runtime_info, image_runtime_debug) =
                    build_image_picker(image_pref);
                app.set_picker(picker);
                app.set_inline_images_enabled(inline_images_enabled);
                if cfg!(debug_assertions) {
                    app.image_runtime_info = Some(image_runtime_info);
                    app.image_runtime_debug = image_runtime_debug;
                }
                if !app.initial_load_started {
                    app.initial_load_started = true;
                    app.start_initial_load();
                }
                app.status = Some(tr("cli-interactive-beta-notice"));
                run_event_loop(terminal, &mut app).await
            }
        }
        Err(error) => Err(error),
    };

    let restore_result = restore_terminal_fn(terminal);
    let restore_succeeded = restore_result.is_ok();
    let ok = session_result.is_ok() && restore_succeeded;
    transaction.set_tag("outcome", if ok { "ok" } else { "error" });
    transaction.finish(ok);

    let result = match restore_result {
        Ok(()) => session_result,
        Err(error) => Err(error),
    };

    TuiSessionOutcome {
        result,
        restore_succeeded,
    }
}

#[cfg(test)]
async fn run_tui_session<B, Restore, Prepare>(
    terminal: &mut Terminal<B>,
    api: ApiClient,
    bootstrap_icons_enabled: bool,
    restore_terminal_fn: Restore,
    prepare_app: Prepare,
) -> Result<(), String>
where
    B: Backend,
    Restore: FnOnce(&mut Terminal<B>) -> Result<(), String>,
    Prepare: FnOnce(&mut App),
{
    run_tui_session_internal(
        terminal,
        api,
        bootstrap_icons_enabled,
        restore_terminal_fn,
        prepare_app,
    )
    .await
    .result
}

fn init_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>, String> {
    enable_raw_mode().map_err(|e| e.to_string())?;
    let mut stdout = io::stdout();
    if let Err(error) = execute!(stdout, EnterAlternateScreen, EnableMouseCapture) {
        let _ = disable_raw_mode();
        return Err(error.to_string());
    }
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend).map_err(|error| {
        let _ = disable_raw_mode();
        let mut stdout = io::stdout();
        let _ = execute!(
            stdout,
            LeaveAlternateScreen,
            DisableMouseCapture,
            crossterm::cursor::Show
        );
        error.to_string()
    })
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<(), String> {
    let mut first_error = None;
    if let Err(error) = disable_raw_mode() {
        first_error.get_or_insert_with(|| error.to_string());
    }
    if let Err(error) = execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    ) {
        first_error.get_or_insert_with(|| error.to_string());
    }
    if let Err(error) = terminal.show_cursor() {
        first_error.get_or_insert_with(|| error.to_string());
    }
    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

struct TerminalCleanupGuard {
    active: bool,
}

impl TerminalCleanupGuard {
    fn new() -> Self {
        Self { active: true }
    }

    fn dismiss(&mut self) {
        self.active = false;
    }
}

impl Drop for TerminalCleanupGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let _ = disable_raw_mode();
        let mut stdout = io::stdout();
        let _ = execute!(
            stdout,
            LeaveAlternateScreen,
            DisableMouseCapture,
            crossterm::cursor::Show
        );
    }
}

fn is_global_quit_key(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
        && key.modifiers.contains(KeyModifiers::CONTROL)
}

fn navigation_action_for_key(code: KeyCode) -> NavigationAction {
    match code {
        KeyCode::Tab => NavigationAction::NextMode,
        KeyCode::BackTab => NavigationAction::PreviousMode,
        KeyCode::Char('1') => NavigationAction::SwitchMode(ViewMode::List),
        KeyCode::Char('2') => NavigationAction::SwitchMode(ViewMode::Kanban),
        KeyCode::Char('3') => NavigationAction::SwitchMode(ViewMode::Calendar),
        KeyCode::PageUp | KeyCode::Char('[') => NavigationAction::MoveMonth(-1),
        KeyCode::PageDown | KeyCode::Char(']') => NavigationAction::MoveMonth(1),
        KeyCode::Left => NavigationAction::MoveHorizontal {
            delta: -1,
            fallback: FocusPane::Lists,
        },
        KeyCode::Right => NavigationAction::MoveHorizontal {
            delta: 1,
            fallback: FocusPane::Items,
        },
        KeyCode::Up => NavigationAction::MoveSelection(-1),
        KeyCode::Down => NavigationAction::MoveSelection(1),
        KeyCode::Enter => NavigationAction::Enter,
        KeyCode::Esc => NavigationAction::Escape,
        KeyCode::Char('?') | KeyCode::F(1) => NavigationAction::Help,
        KeyCode::Home => NavigationAction::EdgeItem(true),
        KeyCode::End => NavigationAction::EdgeItem(false),
        _ => NavigationAction::Ignore,
    }
}

async fn handle_runtime_event<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    event: Event,
) -> Result<bool, String> {
    match event {
        Event::Key(key) => {
            if key.kind != KeyEventKind::Press {
                return Ok(false);
            }
            if is_global_quit_key(key) {
                app.should_quit = true;
                return Ok(true);
            }
            let result = if app.requires_beta_consent() {
                app.handle_beta_consent_key(key);
                Ok(())
            } else if app.requires_legal_consent() {
                app.handle_legal_consent_key(key);
                Ok(())
            } else if app.editor.is_some() {
                app.handle_editor_key(key).await
            } else {
                app.handle_key(key).await
            };
            if let Err(error) = result {
                app.status = Some(error);
            }
            Ok(true)
        }
        Event::Mouse(mouse) => {
            let size = terminal.size().map_err(|e| e.to_string())?;
            let area = Rect::new(0, 0, size.width, size.height);
            let result = if app.requires_beta_consent() {
                app.handle_beta_consent_mouse(mouse, area);
                Ok(())
            } else if app.requires_legal_consent() {
                app.handle_legal_consent_mouse(mouse, area);
                Ok(())
            } else if app.editor.is_some() {
                app.handle_editor_mouse(mouse, area).await
            } else {
                app.handle_mouse(mouse, area).await
            };
            if let Err(error) = result {
                app.status = Some(error);
            }
            Ok(true)
        }
        Event::Resize(_, _) => Ok(true),
        Event::FocusGained | Event::FocusLost | Event::Paste(_) => Ok(false),
    }
}

async fn run_event_loop<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<(), String> {
    let mut dirty = true;
    loop {
        if app.drain_load_messages() {
            dirty = true;
        }
        if dirty {
            terminal
                .draw(|frame| draw_ui(frame, app))
                .map_err(|e| e.to_string())?;
            dirty = false;
        }

        if app.should_quit {
            return Ok(());
        }

        if !event::poll(Duration::from_millis(120)).map_err(|e| e.to_string())? {
            continue;
        }

        loop {
            if handle_runtime_event(terminal, app, event::read().map_err(|e| e.to_string())?).await?
            {
                dirty = true;
            }

            if !event::poll(Duration::from_millis(0)).map_err(|e| e.to_string())? {
                break;
            }
        }
    }
}

impl App {
    fn new(api: ApiClient, bootstrap_icons_enabled: bool) -> Self {
        let (tx, rx) = unbounded_channel();
        Self {
            api,
            tx,
            rx,
            picker: Picker::halfblocks(),
            lists: Vec::new(),
            items: Vec::new(),
            items_cache: HashMap::new(),
            comments_cache: HashMap::new(),
            selected_list: 0,
            selected_item: 0,
            list_scroll: 0,
            item_scroll: 0,
            item_filter: String::default(),
            mode: ViewMode::List,
            focus: FocusPane::Lists,
            status: Some(tr("cli-interactive-beta-notice")),
            inline_images_enabled: false,
            loading_lists: false,
            loading_items_for: None,
            pending_detail_image: None,
            pending_profile_image: None,
            pending_open_image: None,
            pending_list_icons: HashSet::new(),
            failed_list_icons: HashSet::new(),
            last_item_click: None,
            kanban_drag_item: None,
            kanban_drag_source_column: None,
            kanban_drag_target_column: None,
            kanban_drag_started: false,
            calendar_drag_item: None,
            calendar_drag_source_date: None,
            calendar_drag_target_date: None,
            calendar_drag_started: false,
            calendar_selected_date: None,
            calendar_visible_month: None,
            beta_consent_pending: true,
            legal_consent_pending: false,
            legal_accepting: false,
            legal_pending_docs: Vec::new(),
            initial_load_started: false,
            should_quit: false,
            show_help: false,
            editor: None,
            detail_image: None,
            detail_image_note: None,
            profile_name: None,
            profile_photo_url: None,
            profile_image: None,
            list_icon_images: HashMap::new(),
            bootstrap_icons_enabled,
            image_runtime_info: None,
            image_runtime_debug: Vec::new(),
            key_bindings: KeyBindings::from_env(),
            pending_auto_handoff_list_id: None,
        }
    }

    fn set_picker(&mut self, picker: Picker) {
        self.picker = picker;
    }

    fn set_inline_images_enabled(&mut self, enabled: bool) {
        self.inline_images_enabled = enabled;
        if !enabled {
            self.detail_image = None;
            self.detail_image_note = None;
            self.pending_detail_image = None;
            self.profile_image = None;
            self.pending_profile_image = None;
            self.list_icon_images.clear();
            self.pending_list_icons.clear();
        }
    }

    fn start_initial_load(&mut self) {
        self.load_profile_background();
    }

    fn requires_beta_consent(&self) -> bool {
        self.beta_consent_pending
    }

    fn requires_legal_consent(&self) -> bool {
        self.legal_consent_pending
    }

    fn accept_beta_consent(&mut self) {
        if !self.beta_consent_pending {
            return;
        }

        self.beta_consent_pending = false;
        if self.legal_consent_pending {
            self.status = Some(tr_args(
                "tui-legal-consent-pending",
                &[("docs", self.legal_pending_docs.join(", "))],
            ));
        } else {
            self.status = Some(tr("label-lists"));
            self.reload_lists_background();
        }

        if !self.initial_load_started {
            self.initial_load_started = true;
            self.start_initial_load();
        }
    }

    fn decline_beta_consent(&mut self) {
        self.should_quit = true;
    }

    fn accept_legal_consent(&mut self) {
        if !self.legal_consent_pending || self.legal_accepting {
            return;
        }

        self.legal_accepting = true;
        self.status = Some(tr("tui-legal-consent-submitting"));
        let api = self.api.clone();
        self.spawn_load(async move {
            LoadMessage::AcceptTerms {
                result: api
                    .post::<Value, Value>("/accept-terms", &Value::Object(Map::new()))
                    .await,
            }
        });
    }

    fn decline_legal_consent(&mut self) {
        self.should_quit = true;
    }

    fn handle_beta_consent_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => self.accept_beta_consent(),
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('n') | KeyCode::Char('N') => {
                self.decline_beta_consent();
            }
            _ => {}
        }
    }

    fn handle_beta_consent_mouse(&mut self, mouse: MouseEvent, area: Rect) {
        if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            return;
        }

        let layout = beta_consent_layout(area);
        if rect_contains(layout.accept, mouse.column, mouse.row) {
            self.accept_beta_consent();
        } else if rect_contains(layout.decline, mouse.column, mouse.row) {
            self.decline_beta_consent();
        }
    }

    fn handle_legal_consent_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => self.accept_legal_consent(),
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('n') | KeyCode::Char('N')
                if !self.legal_accepting =>
            {
                self.decline_legal_consent();
            }
            _ => {}
        }
    }

    fn handle_legal_consent_mouse(&mut self, mouse: MouseEvent, area: Rect) {
        if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            return;
        }

        let layout = beta_consent_layout(area);
        if rect_contains(layout.accept, mouse.column, mouse.row) {
            self.accept_legal_consent();
        } else if rect_contains(layout.decline, mouse.column, mouse.row) && !self.legal_accepting {
            self.decline_legal_consent();
        }
    }

    fn spawn_load<F>(&self, task: F)
    where
        F: std::future::Future<Output = LoadMessage> + Send + 'static,
    {
        let tx = self.tx.clone();
        let hub = Hub::current();
        tokio::spawn(
            async move {
                let _ = tx.send(task.await);
            }
            .bind_hub(hub),
        );
    }

    fn load_profile_background(&mut self) {
        let api = self.api.clone();
        self.spawn_load(async move {
            let span = crate::telemetry::TraceSpan::child("tui.load", "profile");
            span.set_tag("operation", "profile");
            let result = api.get::<Profile>("/profile").await;
            span.set_status(result.is_ok());
            span.finish();
            LoadMessage::Profile(result)
        });
    }

    fn reload_lists_background(&mut self) {
        self.loading_lists = true;
        self.status = Some(format!("{}...", tr("label-lists")));
        let api = self.api.clone();
        self.spawn_load(async move {
            let span = crate::telemetry::TraceSpan::child("tui.load", "lists");
            span.set_tag("operation", "lists");
            let result = api.get::<Vec<ShoppingList>>("/lists").await;
            if let Ok(lists) = &result {
                span.set_data_i64("result.count", lists.len() as i64);
            }
            span.set_status(result.is_ok());
            span.finish();
            LoadMessage::Lists(result)
        });
    }

    fn drain_load_messages(&mut self) -> bool {
        let mut changed = false;
        while let Ok(message) = self.rx.try_recv() {
            changed = true;
            match message {
                LoadMessage::Profile(result) => self.apply_profile_result(result),
                LoadMessage::Lists(result) => self.apply_lists_result(result),
                LoadMessage::Items { list_id, result } => self.apply_items_result(list_id, result),
                LoadMessage::Comments { item_id, result } => {
                    self.apply_comments_result(item_id, result)
                }
                LoadMessage::DetailImage { source, result } => {
                    self.apply_detail_image_result(source, result)
                }
                LoadMessage::ProfileImage { source, result } => {
                    self.apply_profile_image_result(source, result)
                }
                LoadMessage::ListIcon { icon, result } => self.apply_list_icon_result(icon, result),
                LoadMessage::OpenImage { source, result } => {
                    self.apply_open_image_result(source, result)
                }
                LoadMessage::AcceptTerms { result } => self.apply_accept_terms_result(result),
                LoadMessage::AutoHandoffDue { list_id, list_name } => {
                    if should_send_auto_handoff(
                        self.selected_list_id(),
                        self.pending_auto_handoff_list_id,
                        list_id,
                    ) {
                        self.send_auto_handoff_now_background(list_id, list_name);
                    }
                }
                LoadMessage::AutoHandoffSent => {}
            }
        }
        changed
    }

    fn apply_profile_result(&mut self, result: Result<Profile, String>) {
        if let Ok(profile) = result {
            let pending_docs = profile_pending_legal_docs(&profile);
            self.legal_pending_docs.clone_from(&pending_docs);
            self.legal_consent_pending = !pending_docs.is_empty();
            self.legal_accepting = false;
            self.profile_name = profile.display_name.or(profile.email);
            self.profile_photo_url = profile
                .photo_url
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string);
            self.refresh_profile_image_background();

            if self.beta_consent_pending {
                return;
            }

            if self.legal_consent_pending {
                self.status = Some(tr_args(
                    "tui-legal-consent-pending",
                    &[("docs", self.legal_pending_docs.join(", "))],
                ));
            } else {
                self.reload_lists_background();
            }
        } else if let Err(error) = result {
            self.status = Some(error);
        }
    }

    fn apply_accept_terms_result(&mut self, result: Result<Value, String>) {
        self.legal_accepting = false;
        match result {
            Ok(value) => {
                let pending_docs = pending_legal_docs_from_value(&value);
                self.legal_pending_docs.clone_from(&pending_docs);
                self.legal_consent_pending = !pending_docs.is_empty();
                if self.legal_consent_pending {
                    self.status = Some(tr_args(
                        "tui-legal-consent-pending",
                        &[("docs", pending_docs.join(", "))],
                    ));
                } else {
                    self.status = Some(tr("cli-accepted-terms-all"));
                    self.reload_lists_background();
                }
            }
            Err(error) => {
                self.status = Some(error);
            }
        }
    }

    fn selected_list(&self) -> Option<&ShoppingList> {
        self.lists.get(self.selected_list)
    }

    fn selected_list_id(&self) -> Option<i64> {
        self.selected_list().map(|list| list.id)
    }

    fn selected_list_name(&self) -> String {
        self.selected_list()
            .map_or_else(|| tr("common-unknown"), |list| list.name.clone())
    }

    fn selected_list_display_name(&self) -> String {
        self.selected_list()
            .map_or_else(|| tr("common-unknown"), list_display_name_for_tui)
    }

    fn selected_item(&self) -> Option<&ListItem> {
        self.items.get(self.selected_item)
    }

    fn visible_item_indices(&self) -> Vec<usize> {
        let query = self.item_filter.trim().to_ascii_lowercase();
        if query.is_empty() {
            return (0..self.items.len()).collect();
        }
        self.items
            .iter()
            .enumerate()
            .filter(|(_, item)| item_matches_filter(item, &query))
            .map(|(idx, _)| idx)
            .collect()
    }

    fn selected_visible_position(&self, visible: &[usize]) -> usize {
        visible
            .iter()
            .position(|idx| *idx == self.selected_item)
            .unwrap_or(0)
    }

    fn set_selected_item_by_id(&mut self, id: i64) {
        if let Some(index) = self.items.iter().position(|item| item.id == id) {
            self.selected_item = index;
        }
    }

    fn apply_lists_result(&mut self, result: Result<Vec<ShoppingList>, String>) {
        self.loading_lists = false;
        let previous_list_id = self.selected_list_id();
        let mut lists = match result {
            Ok(lists) => lists,
            Err(error) => {
                self.status = Some(error);
                return;
            }
        };
        if lists.is_empty() {
            self.status = Some(tr("output-no-lists"));
            self.lists.clear();
            self.items.clear();
            return;
        }
        sort_lists_for_tui(&mut lists);
        self.items_cache.clear();
        self.lists = lists;

        self.selected_list = previous_list_id
            .and_then(|id| self.lists.iter().position(|list| list.id == id))
            .unwrap_or(0);

        self.clamp_scrolls();
        self.load_items_for_selected_list(false);
        self.send_auto_handoff_viewing_background();
    }

    fn send_auto_handoff_viewing_background(&mut self) {
        if !auto_handoff_enabled() {
            return;
        }
        let Some(list_id) = self.selected_list_id() else {
            return;
        };
        let list_name = self.selected_list_name();
        self.pending_auto_handoff_list_id = Some(list_id);
        self.spawn_load(async move {
            tokio::time::sleep(Duration::from_millis(350)).await;
            LoadMessage::AutoHandoffDue { list_id, list_name }
        });
    }

    fn send_auto_handoff_now_background(&mut self, list_id: i64, list_name: String) {
        let api = self.api.clone();
        self.spawn_load(async move {
            let mut body = Map::new();
            body.insert("list_id".to_string(), Value::from(list_id));
            body.insert("list_name".to_string(), Value::String(list_name));
            body.insert(
                "device_label".to_string(),
                Value::String(default_handoff_device_label()),
            );
            let _: Result<Value, String> =
                api.post("/activity/viewing", &Value::Object(body)).await;
            LoadMessage::AutoHandoffSent
        });
    }

    fn clear_kanban_drag_state(&mut self) {
        self.kanban_drag_item = None;
        self.kanban_drag_source_column = None;
        self.kanban_drag_target_column = None;
        self.kanban_drag_started = false;
    }

    fn clear_calendar_drag_state(&mut self) {
        self.calendar_drag_item = None;
        self.calendar_drag_source_date = None;
        self.calendar_drag_target_date = None;
        self.calendar_drag_started = false;
    }

    fn clear_drag_state(&mut self) {
        self.clear_kanban_drag_state();
        self.clear_calendar_drag_state();
    }

    fn sync_calendar_date_to_selected_item(&mut self) {
        let date = self.default_calendar_date();
        self.calendar_selected_date = Some(date);
        self.calendar_visible_month = Some(start_of_month(date));
    }

    fn default_calendar_date(&self) -> SimpleDate {
        if let Some(date) = self
            .selected_item()
            .and_then(|item| item.due_date.as_deref())
            .and_then(parse_iso_date)
        {
            return date;
        }
        if let Some(date) = self.calendar_selected_date {
            return date;
        }
        for index in self.visible_item_indices() {
            if let Some(date) = self.items[index]
                .due_date
                .as_deref()
                .and_then(parse_iso_date)
            {
                return date;
            }
        }
        today_utc()
    }

    fn move_calendar_date_selection(&mut self, delta_days: i64) {
        let base = self
            .calendar_selected_date
            .or_else(|| {
                self.selected_item()
                    .and_then(|item| item.due_date.as_deref())
                    .and_then(parse_iso_date)
            })
            .unwrap_or_else(today_utc);
        let next = shifted_date(base, delta_days);
        self.calendar_selected_date = Some(next);
        self.calendar_visible_month = Some(start_of_month(next));
        if let Some(index) = self
            .items
            .iter()
            .position(|item| item.due_date.as_deref().and_then(parse_iso_date) == Some(next))
        {
            self.selected_item = index;
        }
        self.clear_calendar_drag_state();
    }

    fn move_calendar_month_selection(&mut self, delta_months: i32) {
        let base = self
            .calendar_selected_date
            .or(self.calendar_visible_month)
            .unwrap_or_else(|| start_of_month(self.default_calendar_date()));
        let next = shifted_month(base, delta_months);
        self.calendar_selected_date = Some(next);
        self.calendar_visible_month = Some(start_of_month(next));
        if let Some(index) = self
            .items
            .iter()
            .position(|item| item.due_date.as_deref().and_then(parse_iso_date) == Some(next))
        {
            self.selected_item = index;
        }
        self.clear_calendar_drag_state();
    }

    fn pointer_calendar_month(&self) -> SimpleDate {
        self.calendar_visible_month
            .or_else(|| self.calendar_selected_date.map(start_of_month))
            .unwrap_or_else(|| start_of_month(self.default_calendar_date()))
    }

    fn load_items_for_selected_list(&mut self, preserve_visible_items: bool) {
        let previous_item_id = if preserve_visible_items {
            self.selected_item().map(|item| item.id)
        } else {
            None
        };
        let Some(list_id) = self.selected_list_id() else {
            self.items.clear();
            self.selected_item = 0;
            self.detail_image = None;
            self.detail_image_note = None;
            return;
        };

        if let Some(cached) = self.items_cache.get(&list_id) {
            self.items.clone_from(cached);
            self.loading_items_for = None;
            self.apply_item_selection(previous_item_id);
            self.status = Some(format!(
                "{} | {} {}",
                self.selected_list_display_name(),
                self.items.len(),
                tr("label-items")
            ));
            self.refresh_selected_image_background();
            self.load_comments_for_selected_item();
            return;
        }

        self.loading_items_for = Some(list_id);
        if !preserve_visible_items {
            self.items.clear();
            self.selected_item = 0;
            self.item_scroll = 0;
            self.detail_image = None;
            self.detail_image_note = None;
        }
        self.status = Some(tr_args(
            "tui-items-loading",
            &[("list", self.selected_list_display_name())],
        ));
        let api = self.api.clone();
        self.spawn_load(async move {
            let span = crate::telemetry::TraceSpan::child("tui.load", "items");
            span.set_tag("operation", "items");
            LoadMessage::Items {
                list_id,
                result: {
                    let result = api
                        .get::<Vec<ListItem>>(&format!("/lists/{list_id}/items"))
                        .await;
                    if let Ok(items) = &result {
                        span.set_data_i64("result.count", items.len() as i64);
                    }
                    span.set_status(result.is_ok());
                    span.finish();
                    result
                },
            }
        });
    }

    fn apply_items_result(&mut self, list_id: i64, result: Result<Vec<ListItem>, String>) {
        if self.selected_list_id() != Some(list_id) {
            if let Ok(mut items) = result {
                apply_item_depths(&mut items);
                self.items_cache.insert(list_id, items);
            }
            return;
        }

        self.loading_items_for = None;
        match result {
            Ok(mut items) => {
                let previous_item_id = self.selected_item().map(|item| item.id);
                apply_item_depths(&mut items);
                self.items = items;
                self.items_cache.insert(list_id, self.items.clone());
                self.apply_item_selection(previous_item_id);
                self.status = Some(format!(
                    "{} | {} {}",
                    self.selected_list_display_name(),
                    self.items.len(),
                    tr("label-items")
                ));
                self.refresh_selected_image_background();
                self.load_comments_for_selected_item();
            }
            Err(error) => {
                self.items.clear();
                self.selected_item = 0;
                self.item_scroll = 0;
                self.status = Some(error);
            }
        }
    }

    fn apply_item_selection(&mut self, previous_item_id: Option<i64>) {
        self.selected_item = previous_item_id
            .and_then(|id| self.items.iter().position(|item| item.id == id))
            .unwrap_or(0)
            .min(self.items.len().saturating_sub(1));
        self.clamp_scrolls();
    }

    fn reload_items_force_background(&mut self) {
        if let Some(list_id) = self.selected_list_id() {
            self.items_cache.remove(&list_id);
            self.load_items_for_selected_list(true);
        }
    }

    fn refresh_lists_force_background(&mut self) {
        self.items_cache.clear();
        self.comments_cache.clear();
        self.reload_lists_background();
    }

    fn load_comments_for_selected_item(&mut self) {
        let Some(item_id) = self.selected_item().map(|item| item.id) else {
            return;
        };
        if self.comments_cache.contains_key(&item_id) {
            return;
        }
        let api = self.api.clone();
        self.spawn_load(async move {
            let span = crate::telemetry::TraceSpan::child("tui.load", "comments");
            span.set_tag("operation", "comments");
            LoadMessage::Comments {
                item_id,
                result: {
                    let result = api
                        .get::<Vec<ItemComment>>(&format!("/items/{item_id}/comments"))
                        .await;
                    if let Ok(comments) = &result {
                        span.set_data_i64("result.count", comments.len() as i64);
                    }
                    span.set_status(result.is_ok());
                    span.finish();
                    result
                },
            }
        });
    }

    fn apply_comments_result(&mut self, item_id: i64, result: Result<Vec<ItemComment>, String>) {
        if let Ok(comments) = result {
            self.comments_cache.insert(item_id, comments);
        }
    }

    fn clamp_scrolls(&mut self) {
        self.selected_list = self.selected_list.min(self.lists.len().saturating_sub(1));
        self.selected_item = self.selected_item.min(self.items.len().saturating_sub(1));
        let rows = list_panel_rows(&self.lists);
        let selected_row = selected_list_panel_row(&rows, self.selected_list);
        self.list_scroll = self
            .list_scroll
            .min(selected_row)
            .min(rows.len().saturating_sub(1));
        self.item_scroll = self
            .item_scroll
            .min(self.selected_item.saturating_sub(0))
            .min(self.items.len().saturating_sub(1));
    }

    fn reload_items(&mut self) {
        self.load_items_for_selected_list(false);
    }

    fn scroll_active(&mut self, delta: isize) {
        match self.focus {
            FocusPane::Lists => {
                if self.lists.is_empty() {
                    return;
                }
                self.selected_list = shifted_index(self.selected_list, delta, self.lists.len());
                let rows = list_panel_rows(&self.lists);
                let selected_row = selected_list_panel_row(&rows, self.selected_list);
                self.list_scroll = scroll_to_visible(self.list_scroll, selected_row, 8);
                self.load_items_for_selected_list(false);
            }
            FocusPane::Items => {
                if self.mode == ViewMode::Kanban {
                    let step = delta.signum();
                    if step != 0 {
                        let _ = self.move_kanban_selection(step);
                    }
                    return;
                }
                let visible = self.visible_item_indices();
                if visible.is_empty() {
                    return;
                }
                let pos = self.selected_visible_position(&visible);
                let next_pos = shifted_index(pos, delta, visible.len());
                self.selected_item = visible[next_pos];
                self.item_scroll = scroll_to_visible(self.item_scroll, next_pos, 12);
                if self.mode == ViewMode::List {
                    self.refresh_selected_image_background();
                    self.load_comments_for_selected_item();
                }
            }
        }
    }

    fn apply_detail_image_result(&mut self, source: String, result: Result<Vec<u8>, String>) {
        if self.pending_detail_image.as_deref() != Some(source.as_str()) {
            return;
        }
        self.pending_detail_image = None;

        let Ok(bytes) = result else {
            self.detail_image = None;
            self.detail_image_note = Some("—".to_string());
            return;
        };
        let Ok(image) = load_oriented_image(&bytes) else {
            self.detail_image = None;
            self.detail_image_note = Some("—".to_string());
            return;
        };
        let protocol = self.picker.new_resize_protocol(image);
        self.detail_image = Some(DetailImageState { source, protocol });
        self.detail_image_note = None;
    }

    fn apply_profile_image_result(&mut self, source: String, result: Result<Vec<u8>, String>) {
        if self.pending_profile_image.as_deref() != Some(source.as_str()) {
            return;
        }
        self.pending_profile_image = None;

        let Ok(bytes) = result else {
            self.profile_image = None;
            return;
        };
        let Ok(image) = load_oriented_image(&bytes) else {
            self.profile_image = None;
            return;
        };
        let protocol = self.picker.new_resize_protocol(image);
        self.profile_image = Some(DetailImageState { source, protocol });
    }

    fn apply_list_icon_result(&mut self, icon: String, result: Result<DynamicImage, String>) {
        self.pending_list_icons.remove(&icon);
        let Ok(image) = result else {
            self.failed_list_icons.insert(icon);
            return;
        };
        match self.picker.new_protocol(
            image,
            Size::new(2, 1),
            Resize::Fit(Some(FilterType::Lanczos3)),
        ) {
            Ok(protocol) => {
                self.failed_list_icons.remove(&icon);
                self.list_icon_images.insert(icon, protocol);
            }
            Err(_) => {
                self.failed_list_icons.insert(icon);
            }
        }
    }

    fn refresh_selected_image_background(&mut self) {
        if !self.inline_images_enabled {
            self.detail_image = None;
            self.detail_image_note = None;
            self.pending_detail_image = None;
            return;
        }

        let Some(item) = self.selected_item().cloned() else {
            self.detail_image = None;
            self.detail_image_note = None;
            self.pending_detail_image = None;
            return;
        };

        let Some(source) = Self::selected_image_source(&item) else {
            self.detail_image = None;
            self.detail_image_note = None;
            self.pending_detail_image = None;
            return;
        };

        if self
            .detail_image
            .as_ref()
            .is_some_and(|state| state.source == source)
            || self.pending_detail_image.as_deref() == Some(source.as_str())
        {
            return;
        }

        self.detail_image = None;
        self.detail_image_note = Some(format!("{}...", tr("label-image")));
        self.pending_detail_image = Some(source.clone());
        let api = self.api.clone();
        self.spawn_load(async move {
            let span = crate::telemetry::TraceSpan::child("tui.image", "detail_image");
            span.set_tag("operation", "detail_image");
            let result = api.get_bytes(&source).await;
            if let Ok(bytes) = &result {
                span.set_data_i64("image.bytes", bytes.len() as i64);
            }
            span.set_status(result.is_ok());
            span.finish();
            LoadMessage::DetailImage {
                source: source.clone(),
                result,
            }
        });
    }

    fn refresh_profile_image_background(&mut self) {
        if !self.inline_images_enabled {
            self.profile_image = None;
            self.pending_profile_image = None;
            return;
        }
        let Some(source) = self.profile_photo_url.clone() else {
            self.profile_image = None;
            self.pending_profile_image = None;
            return;
        };
        if self
            .profile_image
            .as_ref()
            .is_some_and(|state| state.source == source)
            || self.pending_profile_image.as_deref() == Some(source.as_str())
        {
            return;
        }

        self.pending_profile_image = Some(source.clone());
        let api = self.api.clone();
        self.spawn_load(async move {
            let span = crate::telemetry::TraceSpan::child("tui.image", "profile_image");
            span.set_tag("operation", "profile_image");
            let result = api.get_bytes(&source).await;
            if let Ok(bytes) = &result {
                span.set_data_i64("image.bytes", bytes.len() as i64);
            }
            span.set_status(result.is_ok());
            span.finish();
            LoadMessage::ProfileImage { source, result }
        });
    }

    fn ensure_list_icon_background(&mut self, icon: &str) {
        if !self.bootstrap_icons_enabled
            || !self.inline_images_enabled
            || self.list_icon_images.contains_key(icon)
            || self.pending_list_icons.contains(icon)
            || self.failed_list_icons.contains(icon)
        {
            return;
        }

        let icon_name = icon.to_string();
        self.pending_list_icons.insert(icon_name.clone());
        self.spawn_load(async move {
            let result = fetch_bootstrap_icon_image(&icon_name).await;
            LoadMessage::ListIcon {
                icon: icon_name,
                result,
            }
        });
    }

    fn selected_image_source(item: &ListItem) -> Option<String> {
        if let Some(url) = item
            .image_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(url.to_string());
        }

        if let Some(url) = item
            .attachments
            .as_ref()
            .and_then(|attachments| {
                attachments
                    .iter()
                    .find(|attachment| is_image_attachment(attachment))
            })
            .and_then(|attachment| attachment.url.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
        {
            return Some(url);
        }

        item.notes.as_deref().and_then(extract_note_image_source)
    }

    fn open_selected_image_background(&mut self) -> Result<(), String> {
        let Some(item) = self.selected_item().cloned() else {
            self.status = Some(tr("output-no-items"));
            return Ok(());
        };
        let Some(source) = Self::selected_image_source(&item) else {
            self.status = Some(format!("{} —", tr("label-image")));
            return Ok(());
        };

        if self.inline_images_enabled {
            self.refresh_selected_image_background();
            self.status = Some(if self.detail_image.is_some() {
                tr("label-image")
            } else {
                format!("{}...", tr("label-image"))
            });
            return Ok(());
        }

        if self.pending_open_image.as_deref() == Some(source.as_str()) {
            return Ok(());
        }

        self.pending_open_image = Some(source.clone());
        self.status = Some(format!("{}...", tr("label-image")));
        let api = self.api.clone();
        self.spawn_load(async move {
            let span = crate::telemetry::TraceSpan::child("tui.image", "open_external");
            span.set_tag("operation", "open_external");
            let result = fetch_and_open_image(api, source.clone()).await;
            span.set_status(result.is_ok());
            span.finish();
            LoadMessage::OpenImage {
                source: source.clone(),
                result,
            }
        });
        Ok(())
    }

    fn apply_open_image_result(&mut self, source: String, result: Result<String, String>) {
        if self.pending_open_image.as_deref() != Some(source.as_str()) {
            return;
        }
        self.pending_open_image = None;
        self.status = Some(match result {
            Ok(path) => format!("{}: {path}", tr("label-image")),
            Err(error) => error,
        });
    }

    fn open_editor(&mut self) -> Result<(), String> {
        let Some(item) = self.selected_item() else {
            self.status = Some(tr("output-no-items"));
            return Ok(());
        };

        self.editor = Some(EditorState {
            mode: EditorMode::Edit,
            item_id: Some(item.id),
            text: item.text.clone(),
            quantity: item.quantity.clone().unwrap_or_default(),
            due_date: item.due_date.clone().unwrap_or_default(),
            due_time: item.due_time.clone().unwrap_or_default(),
            planned_date: item.planned_date.clone().unwrap_or_default(),
            planned_time: item.planned_time.clone().unwrap_or_default(),
            reminder: editor_bool_label(item.reminder),
            reminder_time: item.reminder_time.clone().unwrap_or_default(),
            reminder_offsets: item
                .reminder_offsets
                .clone()
                .unwrap_or_default()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", "),
            travel_time_minutes: item
                .travel_time_minutes
                .map(|value| value.to_string())
                .unwrap_or_default(),
            priority: item.priority.clone().unwrap_or_default(),
            tags: item.tags.clone().unwrap_or_default().join(", "),
            progress: item.progress.clone().unwrap_or_default(),
            notes: item
                .notes
                .as_deref()
                .map(note_text_for_editor)
                .unwrap_or_default(),
            active_field: EditorField::Text,
        });
        Ok(())
    }

    fn open_add_editor(&mut self) -> Result<(), String> {
        if self.selected_list_id().is_none() {
            self.status = Some(tr("output-no-lists"));
            return Ok(());
        }
        let due_date = if self.mode == ViewMode::Calendar {
            self.calendar_selected_date
                .map(format_iso_date)
                .unwrap_or_default()
        } else {
            String::default()
        };
        self.editor = Some(EditorState {
            mode: EditorMode::Create,
            item_id: None,
            text: String::default(),
            quantity: String::default(),
            due_date,
            due_time: String::default(),
            planned_date: String::default(),
            planned_time: String::default(),
            reminder: String::default(),
            reminder_time: String::default(),
            reminder_offsets: String::default(),
            travel_time_minutes: String::default(),
            priority: String::default(),
            tags: String::default(),
            progress: self.default_progress_value(),
            notes: String::default(),
            active_field: EditorField::Text,
        });
        Ok(())
    }

    fn open_comment_editor(&mut self) -> Result<(), String> {
        let Some(item_id) = self.selected_item().map(|item| item.id) else {
            self.status = Some(tr("output-no-items"));
            return Ok(());
        };
        self.editor = Some(EditorState {
            mode: EditorMode::Comment,
            item_id: Some(item_id),
            text: String::default(),
            quantity: String::default(),
            due_date: String::default(),
            due_time: String::default(),
            planned_date: String::default(),
            planned_time: String::default(),
            reminder: String::default(),
            reminder_time: String::default(),
            reminder_offsets: String::default(),
            travel_time_minutes: String::default(),
            priority: String::default(),
            tags: String::default(),
            progress: String::default(),
            notes: String::default(),
            active_field: EditorField::Text,
        });
        Ok(())
    }

    fn open_filter_editor(&mut self) -> Result<(), String> {
        self.editor = Some(EditorState {
            mode: EditorMode::Filter,
            item_id: None,
            text: self.item_filter.clone(),
            quantity: String::default(),
            due_date: String::default(),
            due_time: String::default(),
            planned_date: String::default(),
            planned_time: String::default(),
            reminder: String::default(),
            reminder_time: String::default(),
            reminder_offsets: String::default(),
            travel_time_minutes: String::default(),
            priority: String::default(),
            tags: String::default(),
            progress: String::default(),
            notes: String::default(),
            active_field: EditorField::Text,
        });
        Ok(())
    }

    async fn save_editor(&mut self) -> Result<(), String> {
        let Some(editor) = self.editor.as_ref().cloned() else {
            return Ok(());
        };

        if editor.mode == EditorMode::Filter {
            return self.save_filter_editor(&editor);
        }

        let text = editor.text.trim().to_string();
        if text.is_empty() {
            self.status = Some(tr("label-text"));
            return Ok(());
        }

        if editor.mode == EditorMode::Comment {
            return self.save_comment_editor(&editor, &text).await;
        }

        let due_date = editor.due_date.trim().to_string();
        if !valid_due_date_input(&due_date) {
            self.status = Some(format!(
                "{}: YYYY-MM-DD",
                tr("label-due").trim_end_matches(':')
            ));
            return Ok(());
        }

        let current_progress = editor
            .item_id
            .and_then(|item_id| self.items.iter().find(|item| item.id == item_id))
            .and_then(|item| item.progress.as_deref())
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        let progress_raw = editor.progress.trim();
        let Some(progress) = self.normalize_progress_input(progress_raw).or_else(|| {
            (editor.mode == EditorMode::Edit
                && !progress_raw.is_empty()
                && current_progress.eq_ignore_ascii_case(progress_raw))
            .then(|| current_progress.clone())
        }) else {
            self.status = Some(format!(
                "{}: {}",
                tr("label-state").trim_end_matches(':'),
                self.progress_choices().join(" | ")
            ));
            return Ok(());
        };

        let mut body = Map::new();
        body.insert("text".to_string(), Value::String(text));
        body.insert(
            "quantity".to_string(),
            Value::String(editor.quantity.trim().to_string()),
        );
        body.insert("due_date".to_string(), Value::String(due_date));
        body.insert(
            "due_time".to_string(),
            Value::String(editor.due_time.trim().to_string()),
        );
        body.insert(
            "planned_date".to_string(),
            Value::String(editor.planned_date.trim().to_string()),
        );
        body.insert(
            "planned_time".to_string(),
            Value::String(editor.planned_time.trim().to_string()),
        );
        let reminder = parse_editor_bool_input(&editor.reminder);
        let reminder_time = editor.reminder_time.trim();
        let reminder_offsets = parse_i64_csv(&editor.reminder_offsets);
        let travel_time = editor.travel_time_minutes.trim();
        let travel_time_minutes = travel_time.parse::<i64>().ok();
        if let Some(reminder) = reminder.or_else(|| {
            editor_reminder_details_provided(reminder_time, &reminder_offsets).then_some(true)
        }) {
            body.insert("reminder".to_string(), Value::Bool(reminder));
        }
        if !reminder_time.is_empty() {
            body.insert(
                "reminder_time".to_string(),
                Value::String(reminder_time.to_string()),
            );
        }
        if !reminder_offsets.is_empty() {
            body.insert(
                "reminder_offsets".to_string(),
                Value::Array(reminder_offsets.into_iter().map(Value::from).collect()),
            );
        }
        if let Some(minutes) = travel_time_minutes {
            body.insert("travel_time_minutes".to_string(), Value::from(minutes));
        }
        body.insert(
            "priority".to_string(),
            Value::String(editor.priority.trim().to_string()),
        );
        body.insert("tags".to_string(), tags_value(&editor.tags));
        if editor.mode == EditorMode::Create || current_progress != progress {
            body.insert("progress".to_string(), Value::String(progress));
        }
        body.insert(
            "notes".to_string(),
            Value::String(editor.notes.trim().to_string()),
        );

        let updated: ListItem = if editor.mode == EditorMode::Create {
            let Some(list_id) = self.selected_list_id() else {
                return Ok(());
            };
            self.api
                .post(&format!("/lists/{list_id}/items"), &Value::Object(body))
                .await?
        } else {
            let Some(item_id) = editor.item_id else {
                return Ok(());
            };
            self.api
                .put(&format!("/items/{item_id}"), &Value::Object(body))
                .await?
        };

        let updated_id = updated.id;
        if let Some(index) = self.items.iter().position(|item| item.id == updated_id) {
            self.items[index] = updated;
            self.selected_item = index;
        } else {
            self.items.push(updated);
            self.selected_item = self.items.len().saturating_sub(1);
        }

        if let Some(list_id) = self.selected_list_id() {
            self.items_cache.insert(list_id, self.items.clone());
        }

        self.editor = None;
        self.status = Some(if editor.mode == EditorMode::Create {
            tr_args("cli-item-created", &[("id", updated_id.to_string())])
        } else {
            tr("cli-item-updated")
        });
        self.refresh_selected_image_background();
        self.load_comments_for_selected_item();
        Ok(())
    }

    fn save_filter_editor(&mut self, editor: &EditorState) -> Result<(), String> {
        self.item_filter = editor.text.trim().to_string();
        self.selected_item = 0;
        self.item_scroll = 0;
        self.editor = None;
        if let Some(first) = self.visible_item_indices().first().copied() {
            self.selected_item = first;
        }
        self.status = if self.item_filter.is_empty() {
            Some(tr("label-items"))
        } else {
            Some(tr_args(
                "tui-filter-status",
                &[("query", self.item_filter.clone())],
            ))
        };
        self.refresh_selected_image_background();
        self.load_comments_for_selected_item();
        Ok(())
    }

    async fn save_comment_editor(
        &mut self,
        editor: &EditorState,
        text: &str,
    ) -> Result<(), String> {
        let Some(item_id) = editor.item_id else {
            return Ok(());
        };
        let comment: ItemComment = self
            .api
            .post(
                &format!("/items/{item_id}/comments"),
                &serde_json::json!({ "text": text }),
            )
            .await?;
        self.comments_cache
            .entry(item_id)
            .or_default()
            .push(comment);
        self.editor = None;
        self.status = Some(tr("label-comments"));
        Ok(())
    }

    async fn toggle_selected_done(&mut self) -> Result<(), String> {
        let Some(item_id) = self.selected_item().map(|item| item.id) else {
            return Ok(());
        };

        let updated: ListItem = self
            .api
            .patch_json(
                &format!("/items/{item_id}/done"),
                &Value::Object(Map::new()),
            )
            .await?;
        if let Some(item) = self.items.iter_mut().find(|item| item.id == item_id) {
            *item = updated;
        }
        if let Some(list_id) = self.selected_list_id() {
            self.items_cache.insert(list_id, self.items.clone());
        }
        self.set_selected_item_by_id(item_id);
        self.status = Some(tr("cli-item-toggled"));
        Ok(())
    }

    async fn delete_selected_item(&mut self) -> Result<(), String> {
        let Some(item_id) = self.selected_item().map(|item| item.id) else {
            return Ok(());
        };

        let _: Value = self.api.delete(&format!("/items/{item_id}")).await?;
        if let Some(index) = self.items.iter().position(|item| item.id == item_id) {
            self.items.remove(index);
            self.selected_item = index.min(self.items.len().saturating_sub(1));
            self.item_scroll = scroll_to_visible(self.item_scroll, self.selected_item, 12);
            if let Some(list_id) = self.selected_list_id() {
                self.items_cache.insert(list_id, self.items.clone());
            }
        } else {
            self.reload_items_force_background();
        }
        self.refresh_selected_image_background();
        self.status = Some(tr("cli-item-deleted"));
        Ok(())
    }

    async fn show_members_summary(&mut self) -> Result<(), String> {
        let Some(list_id) = self.selected_list_id() else {
            return Ok(());
        };
        let members: Vec<crate::models::Member> =
            self.api.get(&format!("/lists/{list_id}/members")).await?;
        let summary = if members.is_empty() {
            tr("output-no-members")
        } else {
            let preview = members
                .iter()
                .take(3)
                .map(|member| {
                    member
                        .display_name
                        .as_deref()
                        .or(member.email.as_deref())
                        .unwrap_or("?")
                        .to_string()
                })
                .collect::<Vec<_>>()
                .join(", ");
            if members.len() > 3 {
                format!(
                    "{}: {} · {} +{}",
                    tr("member-type-member"),
                    members.len(),
                    preview,
                    members.len() - 3
                )
            } else {
                format!(
                    "{}: {} · {}",
                    tr("member-type-member"),
                    members.len(),
                    preview
                )
            }
        };
        self.status = Some(summary);
        Ok(())
    }

    async fn create_invite_link(&mut self) -> Result<(), String> {
        let Some(list_id) = self.selected_list_id() else {
            return Ok(());
        };
        let resp: Value = self
            .api
            .post(
                &format!("/lists/{list_id}/invite-link"),
                &Value::Object(Map::new()),
            )
            .await?;
        if let Some(url) = invite_url_from_response(&resp) {
            self.status = Some(tr_args("cli-invite-link", &[("url", url)]));
        } else {
            self.status = Some(tr_args("cli-invite-link", &[("url", "-".to_string())]));
        }
        Ok(())
    }

    async fn undo_selected_list(&mut self) -> Result<(), String> {
        let Some(list_id) = self.selected_list_id() else {
            return Ok(());
        };
        let _: Value = self
            .api
            .post(
                &format!("/lists/{list_id}/undo"),
                &Value::Object(Map::new()),
            )
            .await?;
        self.status = Some(tr("cli-undo-done"));
        self.reload_items_force_background();
        Ok(())
    }

    async fn trigger_footer_action(&mut self, action: FooterAction) -> Result<(), String> {
        if matches!(
            action,
            FooterAction::Refresh
                | FooterAction::ToggleDone
                | FooterAction::Delete
                | FooterAction::OpenImage
                | FooterAction::Undo
                | FooterAction::Members
                | FooterAction::Invite
        ) {
            self.status = Some(format!(
                "{}...",
                action_chip_text(action, &self.key_bindings)
            ));
        }

        match action {
            FooterAction::Add => self.open_add_editor(),
            FooterAction::Refresh => {
                self.refresh_lists_force_background();
                Ok(())
            }
            FooterAction::Filter => self.open_filter_editor(),
            FooterAction::Edit => self.open_editor(),
            FooterAction::ToggleDone => self.toggle_selected_done().await,
            FooterAction::Delete => self.delete_selected_item().await,
            FooterAction::OpenImage => self.open_selected_image_background(),
            FooterAction::Comment => self.open_comment_editor(),
            FooterAction::Undo => self.undo_selected_list().await,
            FooterAction::Members => self.show_members_summary().await,
            FooterAction::Invite => self.create_invite_link().await,
            FooterAction::Help => {
                self.show_help = true;
                Ok(())
            }
            FooterAction::Quit => {
                self.should_quit = true;
                Ok(())
            }
        }
    }

    async fn handle_key(&mut self, key: KeyEvent) -> Result<(), String> {
        if self.handle_help_key(key) {
            return Ok(());
        }

        if let Some(action) = self.key_bindings.action_for_key(key) {
            self.trigger_footer_action(action).await?;
            return Ok(());
        }

        let (list_changed, item_changed) = self.handle_navigation_key(key).await?;
        self.apply_key_change_effects(list_changed, item_changed);
        Ok(())
    }

    fn handle_help_key(&mut self, key: KeyEvent) -> bool {
        if !self.show_help {
            return false;
        }

        if matches!(
            key.code,
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') | KeyCode::F(1)
        ) {
            self.show_help = false;
        }
        true
    }

    async fn handle_navigation_key(&mut self, key: KeyEvent) -> Result<(bool, bool), String> {
        let mut list_changed = false;
        let mut item_changed = false;

        match navigation_action_for_key(key.code) {
            NavigationAction::NextMode => self.switch_mode(self.mode.next()),
            NavigationAction::PreviousMode => self.switch_mode(self.mode.prev()),
            NavigationAction::SwitchMode(mode) => self.switch_mode(mode),
            NavigationAction::MoveMonth(delta) => self.move_month_if_calendar_items(delta),
            NavigationAction::MoveHorizontal { delta, fallback } => {
                self.move_horizontal_or_focus(delta, fallback);
            }
            NavigationAction::MoveSelection(delta) => {
                self.move_selection_by_key(key, delta, &mut list_changed, &mut item_changed)
                    .await?;
            }
            NavigationAction::Enter => list_changed = self.handle_enter_key(key)?,
            NavigationAction::Escape => self.handle_escape_key(),
            NavigationAction::Help => self.show_help = true,
            NavigationAction::EdgeItem(first) => {
                item_changed = self.select_visible_edge_item(first)
            }
            NavigationAction::Ignore => {}
        }

        Ok((list_changed, item_changed))
    }

    fn switch_mode(&mut self, mode: ViewMode) {
        self.mode = mode;
        self.clear_kanban_drag_state();
        self.clear_calendar_drag_state();
        if self.mode == ViewMode::Calendar {
            self.sync_calendar_date_to_selected_item();
        }
    }

    fn move_month_if_calendar_items(&mut self, delta: i32) {
        if self.mode == ViewMode::Calendar && self.focus == FocusPane::Items {
            self.move_calendar_month_selection(delta);
        }
    }

    fn move_horizontal_or_focus(&mut self, delta: i64, fallback: FocusPane) {
        if self.mode == ViewMode::Calendar && self.focus == FocusPane::Items {
            self.move_calendar_date_selection(delta);
        } else {
            self.focus = fallback;
        }
    }

    async fn move_selection_by_key(
        &mut self,
        key: KeyEvent,
        delta: isize,
        list_changed: &mut bool,
        item_changed: &mut bool,
    ) -> Result<(), String> {
        match self.focus {
            FocusPane::Lists => *list_changed = self.move_selected_list(delta),
            FocusPane::Items => {
                *item_changed = self.move_selected_item_by_key(key, delta).await?;
            }
        }
        Ok(())
    }

    fn move_selected_list(&mut self, delta: isize) -> bool {
        if self.lists.is_empty() {
            return false;
        }
        let new_index = wrapped_index(self.selected_list, delta, self.lists.len());
        if new_index == self.selected_list {
            return false;
        }
        self.selected_list = new_index;
        true
    }

    async fn move_selected_item_by_key(
        &mut self,
        key: KeyEvent,
        delta: isize,
    ) -> Result<bool, String> {
        if self.mode == ViewMode::Calendar {
            return self.move_calendar_selection_by_key(key, delta).await;
        }
        if self.mode == ViewMode::Kanban {
            return Ok(self.move_kanban_selection_wrapped(delta));
        }
        Ok(self.move_visible_list_selection(delta))
    }

    async fn move_calendar_selection_by_key(
        &mut self,
        key: KeyEvent,
        delta: isize,
    ) -> Result<bool, String> {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return self.move_selected_item_calendar_hours(delta as i32).await;
        }
        self.move_calendar_date_selection(delta as i64 * 7);
        Ok(false)
    }

    fn move_visible_list_selection(&mut self, delta: isize) -> bool {
        let visible = self.visible_item_indices();
        if visible.is_empty() {
            return false;
        }
        let pos = self.selected_visible_position(&visible);
        let next_pos = wrapped_index(pos, delta, visible.len());
        if next_pos == pos {
            return false;
        }
        self.selected_item = visible[next_pos];
        true
    }

    fn handle_enter_key(&mut self, key: KeyEvent) -> Result<bool, String> {
        if matches!(self.focus, FocusPane::Lists) {
            self.focus = FocusPane::Items;
            return Ok(true);
        }
        if self.mode == ViewMode::Calendar && self.calendar_selected_date.is_some() {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                self.open_editor()?;
            } else {
                self.open_add_editor()?;
            }
            return Ok(false);
        }
        self.open_editor()?;
        Ok(false)
    }

    fn handle_escape_key(&mut self) {
        if self.mode == ViewMode::Calendar && self.calendar_drag_item.is_some() {
            self.clear_calendar_drag_state();
        } else if self.mode == ViewMode::Calendar && self.calendar_selected_date.is_some() {
            self.calendar_selected_date = None;
            self.clear_calendar_drag_state();
        }
    }

    fn select_visible_edge_item(&mut self, first: bool) -> bool {
        let visible = self.visible_item_indices();
        let target = if first {
            visible.first().copied()
        } else {
            visible.last().copied()
        };
        let Some(target) = target else {
            return false;
        };
        if self.selected_item == target {
            return false;
        }
        self.selected_item = target;
        true
    }

    fn apply_key_change_effects(&mut self, list_changed: bool, item_changed: bool) {
        if list_changed {
            self.calendar_selected_date = None;
            self.calendar_visible_month = None;
            self.reload_items();
            self.send_auto_handoff_viewing_background();
        } else if item_changed && self.mode == ViewMode::List {
            self.refresh_selected_image_background();
            self.load_comments_for_selected_item();
        } else if item_changed && self.mode == ViewMode::Calendar {
            self.sync_calendar_date_to_selected_item();
        }
    }

    async fn handle_editor_key(&mut self, key: KeyEvent) -> Result<(), String> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('s')) {
            return self.save_editor().await;
        }

        let Some(mode) = self.editor.as_ref().map(|editor| editor.mode) else {
            return Ok(());
        };
        let simple_text_dialog = matches!(mode, EditorMode::Comment | EditorMode::Filter);

        match key.code {
            KeyCode::Esc => {
                self.editor = None;
            }
            KeyCode::Tab => self.move_editor_next_if_form(simple_text_dialog),
            KeyCode::BackTab => self.move_editor_prev_if_form(simple_text_dialog),
            KeyCode::Up => self.move_editor_with_suggestion(simple_text_dialog, -1),
            KeyCode::Down => self.move_editor_with_suggestion(simple_text_dialog, 1),
            KeyCode::Left => self.move_editor_prev_if_form(simple_text_dialog),
            KeyCode::Right => self.move_editor_next_if_form(simple_text_dialog),
            KeyCode::Enter => {
                return self.handle_editor_enter(simple_text_dialog).await;
            }
            KeyCode::Backspace => self.pop_editor_value(),
            KeyCode::Delete => self.clear_editor_value(),
            KeyCode::Char(ch)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.push_editor_char(ch);
            }
            _ => {}
        }

        Ok(())
    }

    fn move_editor_next_if_form(&mut self, simple_text_dialog: bool) {
        if simple_text_dialog {
            return;
        }
        if let Some(editor) = self.editor.as_mut() {
            editor_move_next(editor);
        }
    }

    fn move_editor_prev_if_form(&mut self, simple_text_dialog: bool) {
        if simple_text_dialog {
            return;
        }
        if let Some(editor) = self.editor.as_mut() {
            editor_move_prev(editor);
        }
    }

    fn move_editor_with_suggestion(&mut self, simple_text_dialog: bool, delta: isize) {
        if simple_text_dialog || self.apply_editor_suggestion(delta) {
            return;
        }
        if delta < 0 {
            self.move_editor_prev_if_form(false);
        } else {
            self.move_editor_next_if_form(false);
        }
    }

    async fn handle_editor_enter(&mut self, simple_text_dialog: bool) -> Result<(), String> {
        if simple_text_dialog || self.editor_at_last_field() {
            return self.save_editor().await;
        }
        self.move_editor_next_if_form(false);
        Ok(())
    }

    fn editor_at_last_field(&self) -> bool {
        self.editor
            .as_ref()
            .is_some_and(|editor| editor_step_index(editor) + 1 >= editor_fields(editor.mode).len())
    }

    fn pop_editor_value(&mut self) {
        if let Some(editor) = self.editor.as_mut() {
            active_editor_value_mut(editor).pop();
        }
    }

    fn clear_editor_value(&mut self) {
        if let Some(editor) = self.editor.as_mut() {
            active_editor_value_mut(editor).clear();
        }
    }

    fn push_editor_char(&mut self, ch: char) {
        let Some((field, candidate)) = self.editor.as_ref().map(|editor| {
            let mut candidate = active_editor_value(editor).clone();
            candidate.push(ch);
            (editor.active_field, candidate)
        }) else {
            return;
        };

        let allowed = match field {
            EditorField::DueDate => due_date_input_prefix_allowed(&candidate),
            EditorField::Progress => {
                let candidate = candidate.trim();
                candidate.is_empty()
                    || self.progress_choices().iter().any(|choice| {
                        choice
                            .trim()
                            .to_ascii_lowercase()
                            .starts_with(&candidate.to_ascii_lowercase())
                    })
            }
            _ => true,
        };

        if allowed {
            if let Some(editor) = self.editor.as_mut() {
                active_editor_value_mut(editor).push(ch);
            }
        } else {
            self.status = Some(match field {
                EditorField::DueDate => {
                    format!("{}: YYYY-MM-DD", tr("label-due").trim_end_matches(':'))
                }
                EditorField::Progress => format!(
                    "{}: {}",
                    tr("label-state").trim_end_matches(':'),
                    self.progress_choices().join(" | ")
                ),
                _ => "Invalid input".to_string(),
            });
        }
    }

    async fn handle_mouse(&mut self, mouse: MouseEvent, area: Rect) -> Result<(), String> {
        if self.handle_help_mouse(mouse) {
            return Ok(());
        }

        if self.handle_mouse_scroll(mouse) {
            return Ok(());
        }

        let Some(buttons) = MouseButtons::from_kind(mouse.kind) else {
            return Ok(());
        };
        let previous_item = self.selected_item;
        let layout = ui_layout(area);

        if buttons.left_up && !rect_contains(layout.content, mouse.column, mouse.row) {
            self.clear_drag_state();
        }

        if self.handle_tab_mouse(mouse, &layout, buttons.left_down) {
            return Ok(());
        }
        if self
            .handle_footer_mouse(mouse, layout.footer, buttons.left_down)
            .await?
        {
            return Ok(());
        }

        if self.handle_list_panel_mouse(mouse, layout.lists, buttons.left_down) {
            return Ok(());
        }

        if self.handle_non_content_mouse(mouse, &layout, buttons.left_up) {
            return Ok(());
        }

        self.focus = FocusPane::Items;
        if self.handle_empty_items_mouse(buttons.left_up) {
            return Ok(());
        }

        self.handle_mode_mouse(mouse, layout.content, buttons)
            .await?;

        if buttons.left_up {
            self.clear_drag_state();
        }

        if self.selected_item != previous_item && self.mode == ViewMode::List {
            self.refresh_selected_image_background();
            self.load_comments_for_selected_item();
        }

        Ok(())
    }

    fn handle_help_mouse(&mut self, mouse: MouseEvent) -> bool {
        if !self.show_help {
            return false;
        }
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            self.show_help = false;
        }
        true
    }

    fn handle_mouse_scroll(&mut self, mouse: MouseEvent) -> bool {
        let delta = match mouse.kind {
            MouseEventKind::ScrollUp => -1,
            MouseEventKind::ScrollDown => 1,
            _ => return false,
        };
        self.apply_mouse_scroll(delta, mouse.modifiers);
        true
    }

    fn apply_mouse_scroll(&mut self, delta: i32, modifiers: KeyModifiers) {
        if self.focus == FocusPane::Items && self.mode == ViewMode::Calendar {
            self.move_calendar_month_selection(delta);
        } else if self.focus == FocusPane::Items
            && self.mode == ViewMode::Kanban
            && modifiers.contains(KeyModifiers::SHIFT)
        {
            let _ = self.move_kanban_column_selection(delta as isize);
        } else {
            self.scroll_active((delta * 3) as isize);
        }
    }

    fn handle_tab_mouse(&mut self, mouse: MouseEvent, layout: &UiLayout, left_down: bool) -> bool {
        if !left_down {
            return false;
        }
        let Some(idx) = layout
            .tab_chunks
            .iter()
            .position(|rect| rect_contains(*rect, mouse.column, mouse.row))
        else {
            return false;
        };

        self.mode = match idx {
            0 => ViewMode::List,
            1 => ViewMode::Kanban,
            _ => ViewMode::Calendar,
        };
        self.clear_drag_state();
        true
    }

    async fn handle_footer_mouse(
        &mut self,
        mouse: MouseEvent,
        footer: Rect,
        left_down: bool,
    ) -> Result<bool, String> {
        if !left_down {
            return Ok(false);
        }
        let Some(action) = footer_buttons(footer, &self.key_bindings)
            .into_iter()
            .find_map(|(action, rect)| {
                rect_contains(rect, mouse.column, mouse.row).then_some(action)
            })
        else {
            return Ok(false);
        };

        self.trigger_footer_action(action).await?;
        Ok(true)
    }

    fn handle_list_panel_mouse(&mut self, mouse: MouseEvent, lists: Rect, left_down: bool) -> bool {
        let list_rows_area = item_rows_area(list_panel_rows_area(lists));
        if left_down && rect_contains(list_rows_area, mouse.column, mouse.row) {
            self.select_list_panel_row(mouse, list_rows_area);
            self.clear_drag_state();
            return true;
        }
        false
    }

    fn select_list_panel_row(&mut self, mouse: MouseEvent, list_rows_area: Rect) {
        self.focus = FocusPane::Lists;
        let panel_rows = list_panel_rows(&self.lists);
        let row = self.list_scroll + mouse.row.saturating_sub(list_rows_area.y) as usize;
        let Some(list_index) = panel_rows.get(row).and_then(|row| row.list_index) else {
            return;
        };

        self.selected_list = list_index;
        self.calendar_selected_date = None;
        self.calendar_visible_month = None;
        self.reload_items();
        self.send_auto_handoff_viewing_background();
        self.focus = FocusPane::Items;
    }

    fn handle_non_content_mouse(
        &mut self,
        mouse: MouseEvent,
        layout: &UiLayout,
        left_up: bool,
    ) -> bool {
        let in_lists = rect_contains(layout.lists, mouse.column, mouse.row);
        let in_content = rect_contains(layout.content, mouse.column, mouse.row);
        if !in_lists && in_content {
            return false;
        }
        if left_up {
            self.clear_drag_state();
        }
        true
    }

    fn handle_empty_items_mouse(&mut self, left_up: bool) -> bool {
        if !self.items.is_empty() {
            return false;
        }
        if left_up {
            self.clear_drag_state();
        }
        self.status = Some(item_placeholder(self));
        true
    }

    async fn handle_mode_mouse(
        &mut self,
        mouse: MouseEvent,
        content: Rect,
        buttons: MouseButtons,
    ) -> Result<(), String> {
        match self.mode {
            ViewMode::List => self.handle_list_mode_mouse(mouse, content, buttons.left_down),
            ViewMode::Kanban => {
                self.handle_kanban_mode_mouse(
                    mouse,
                    content,
                    buttons.left_down,
                    buttons.left_drag,
                    buttons.left_up,
                )
                .await
            }
            ViewMode::Calendar => {
                self.handle_calendar_mode_mouse(
                    mouse,
                    content,
                    buttons.left_down,
                    buttons.left_drag,
                    buttons.left_up,
                )
                .await
            }
        }
    }

    fn handle_list_mode_mouse(
        &mut self,
        mouse: MouseEvent,
        content: Rect,
        left_down: bool,
    ) -> Result<(), String> {
        let (list_rect, detail_rect) = list_mode_layout(content);
        if left_down && rect_contains(detail_rect, mouse.column, mouse.row) {
            self.open_selected_image_background()?;
            return Ok(());
        }

        let list_items_area = item_rows_area(list_rect);
        if left_down && rect_contains(list_items_area, mouse.column, mouse.row) {
            self.select_list_mode_item_at(mouse, list_rect, list_items_area)?;
        }

        Ok(())
    }

    fn select_list_mode_item_at(
        &mut self,
        mouse: MouseEvent,
        list_rect: Rect,
        list_items_area: Rect,
    ) -> Result<(), String> {
        let visible = self.visible_item_indices();
        let row = mouse.row.saturating_sub(list_items_area.y) as usize;
        let row_width = list_rect.width.saturating_sub(2) as usize;
        let Some(clicked) =
            visible_item_at_wrapped_row(&self.items, &visible, self.item_scroll, row, row_width)
        else {
            return Ok(());
        };

        self.selected_item = clicked;
        if self.register_item_click(clicked) {
            self.open_editor()?;
        }

        Ok(())
    }

    async fn handle_kanban_mode_mouse(
        &mut self,
        mouse: MouseEvent,
        content: Rect,
        left_down: bool,
        left_drag: bool,
        left_up: bool,
    ) -> Result<(), String> {
        let (columns, buckets) = self.kanban_buckets();
        if columns.is_empty() {
            return Ok(());
        }

        let selected_column = buckets
            .iter()
            .position(|bucket| bucket.contains(&self.selected_item))
            .unwrap_or(0);
        let (start_col, visible_count) =
            kanban_visible_range(content, columns.len(), selected_column);
        let chunks = kanban_chunks(content, visible_count);
        let hovered_column =
            kanban_column_at(&chunks, mouse.column, mouse.row).map(|index| index + start_col);

        if left_drag {
            self.update_kanban_drag_target(hovered_column);
            return Ok(());
        }

        if left_up && self.finish_kanban_drag(hovered_column).await? {
            return Ok(());
        }

        if left_down {
            self.start_kanban_mouse_selection(mouse, start_col, &chunks, &buckets)?;
        }

        Ok(())
    }

    fn update_kanban_drag_target(&mut self, hovered_column: Option<usize>) {
        if self.kanban_drag_item.is_some() {
            self.kanban_drag_started = true;
            self.kanban_drag_target_column = hovered_column;
        }
    }

    async fn finish_kanban_drag(&mut self, hovered_column: Option<usize>) -> Result<bool, String> {
        let Some(item_index) = self.kanban_drag_item.take() else {
            return Ok(false);
        };

        let source_column = self.kanban_drag_source_column.take();
        let drag_started = self.kanban_drag_started;
        self.kanban_drag_started = false;
        let target_column = self.kanban_drag_target_column.take().or(hovered_column);

        if drag_started {
            if let Some(target_column) = target_column {
                if source_column != Some(target_column) {
                    self.move_item_to_kanban_column(item_index, target_column)
                        .await?;
                    return Ok(true);
                }
            }
        }

        Ok(false)
    }

    fn start_kanban_mouse_selection(
        &mut self,
        mouse: MouseEvent,
        start_col: usize,
        chunks: &[Rect],
        buckets: &[Vec<usize>],
    ) -> Result<(), String> {
        for (local_index, rect) in chunks.iter().enumerate() {
            let col_index = start_col + local_index;
            let column_items_area = item_rows_area(*rect);
            if !rect_contains(column_items_area, mouse.column, mouse.row) {
                continue;
            }

            self.select_kanban_column_item(mouse, *rect, column_items_area, col_index, buckets)?;
            break;
        }

        Ok(())
    }

    fn select_kanban_column_item(
        &mut self,
        mouse: MouseEvent,
        column_rect: Rect,
        column_items_area: Rect,
        col_index: usize,
        buckets: &[Vec<usize>],
    ) -> Result<(), String> {
        let row = mouse.row.saturating_sub(column_items_area.y) as usize;
        let max_rows = column_rect.height.saturating_sub(2) as usize;
        let total = buckets[col_index].len();
        let (start, item_count, show_top, _show_bottom) =
            kanban_window(&buckets[col_index], self.selected_item, max_rows);
        let item_row_offset = if show_top { 1 } else { 0 };

        if row >= item_row_offset && row < item_row_offset + item_count {
            let clicked_item = buckets[col_index][start + row - item_row_offset];
            self.selected_item = clicked_item;
            if self.register_item_click(clicked_item) {
                self.open_editor()?;
                return Ok(());
            }
            self.kanban_drag_item = Some(clicked_item);
            self.kanban_drag_source_column = Some(col_index);
            self.kanban_drag_target_column = Some(col_index);
            self.kanban_drag_started = false;
        } else if total > start + item_count {
            self.clear_kanban_drag_state();
            self.kanban_drag_target_column = Some(col_index);
        }

        Ok(())
    }

    async fn handle_calendar_mode_mouse(
        &mut self,
        mouse: MouseEvent,
        content: Rect,
        left_down: bool,
        left_drag: bool,
        left_up: bool,
    ) -> Result<(), String> {
        if left_drag {
            self.update_calendar_drag_target(content, mouse);
            return Ok(());
        }

        if left_up {
            self.finish_calendar_drag(content, mouse).await?;
            return Ok(());
        }

        if left_down {
            self.handle_calendar_click(content, mouse).await?;
        }

        Ok(())
    }

    fn update_calendar_drag_target(&mut self, content: Rect, mouse: MouseEvent) {
        if self.calendar_drag_item.is_some() {
            let month = self.pointer_calendar_month();
            self.calendar_drag_started = true;
            self.calendar_drag_target_date =
                calendar_pointer_date(content, mouse.column, mouse.row, month);
        }
    }

    async fn finish_calendar_drag(
        &mut self,
        content: Rect,
        mouse: MouseEvent,
    ) -> Result<(), String> {
        if self.calendar_drag_started {
            self.finish_started_calendar_drag(content, mouse).await?;
        } else if self.calendar_drag_item.is_some() {
            self.status = Some(tr("tui-help-calendar-3"));
        }

        Ok(())
    }

    async fn finish_started_calendar_drag(
        &mut self,
        content: Rect,
        mouse: MouseEvent,
    ) -> Result<(), String> {
        if let Some(item_index) = self.calendar_drag_item.take() {
            let source_date = self.calendar_drag_source_date.take();
            self.calendar_drag_started = false;
            let month = self.pointer_calendar_month();
            let hovered_date = calendar_pointer_date(content, mouse.column, mouse.row, month);
            let target_date = self.calendar_drag_target_date.take().or(hovered_date);

            if let Some(target_date) = target_date {
                self.calendar_selected_date = Some(target_date);
                self.calendar_visible_month = Some(start_of_month(target_date));
                if source_date != Some(target_date) {
                    self.move_item_to_calendar_date(item_index, target_date)
                        .await?;
                    self.clear_calendar_drag_state();
                    return Ok(());
                }
            }
        }

        self.clear_calendar_drag_state();
        Ok(())
    }

    async fn handle_calendar_click(
        &mut self,
        content: Rect,
        mouse: MouseEvent,
    ) -> Result<(), String> {
        let calendar = self.calendar_layout(content);
        if self.handle_calendar_agenda_header_click(&calendar, mouse) {
            return Ok(());
        }

        if let Some(item_index) = calendar_item_at(&calendar, mouse.column, mouse.row) {
            let clicked_agenda = rect_contains(calendar.agenda_area, mouse.column, mouse.row);
            self.handle_calendar_item_click(item_index, clicked_agenda)?;
        } else if let Some(date) = calendar_date_at(&calendar, mouse.column, mouse.row) {
            self.handle_calendar_date_click(date).await?;
        } else if self.calendar_clicked_empty_agenda(&calendar, mouse) {
            self.clear_calendar_drag_state();
            self.open_add_editor()?;
        }

        Ok(())
    }

    fn handle_calendar_agenda_header_click(
        &mut self,
        calendar: &CalendarLayout,
        mouse: MouseEvent,
    ) -> bool {
        let clicked_agenda = rect_contains(calendar.agenda_area, mouse.column, mouse.row);
        let clicked_header = clicked_agenda && mouse.row == calendar.agenda_area.y;
        if !clicked_header || self.calendar_selected_date.is_none() {
            return false;
        }

        self.calendar_selected_date = None;
        self.clear_calendar_drag_state();
        self.status = Some(tr("tui-help-calendar-2"));
        true
    }

    fn handle_calendar_item_click(
        &mut self,
        item_index: usize,
        clicked_agenda: bool,
    ) -> Result<(), String> {
        self.selected_item = item_index;
        if self.register_item_click(item_index) {
            self.clear_calendar_drag_state();
            self.open_editor()?;
            return Ok(());
        }

        if clicked_agenda {
            self.start_calendar_item_drag(item_index);
        } else {
            self.select_calendar_item_date(item_index);
        }

        Ok(())
    }

    fn start_calendar_item_drag(&mut self, item_index: usize) {
        self.calendar_drag_item = Some(item_index);
        self.calendar_drag_source_date = self.items[item_index]
            .due_date
            .as_deref()
            .and_then(parse_iso_date);
        self.calendar_drag_target_date = None;
        self.calendar_drag_started = false;
        self.status = Some(tr("tui-help-calendar-3"));
    }

    fn select_calendar_item_date(&mut self, item_index: usize) {
        if let Some(date) = self.items[item_index]
            .due_date
            .as_deref()
            .and_then(parse_iso_date)
        {
            self.calendar_selected_date = Some(date);
            self.calendar_visible_month = Some(start_of_month(date));
        }
        self.clear_calendar_drag_state();
    }

    async fn handle_calendar_date_click(&mut self, date: SimpleDate) -> Result<(), String> {
        if let Some(item_index) = self.calendar_drag_item {
            self.selected_item = item_index;
            self.calendar_selected_date = Some(date);
            self.calendar_visible_month = Some(start_of_month(date));
            if self.calendar_drag_source_date != Some(date) {
                self.move_item_to_calendar_date(item_index, date).await?;
            }
            self.clear_calendar_drag_state();
            return Ok(());
        }

        if self.calendar_selected_date == Some(date) {
            self.calendar_selected_date = None;
            self.status = Some(tr("tui-help-calendar-2"));
        } else {
            self.calendar_selected_date = Some(date);
            self.status = Some(tr("tui-help-calendar-3"));
        }

        self.calendar_visible_month = Some(start_of_month(date));
        self.clear_calendar_drag_state();
        Ok(())
    }

    fn calendar_clicked_empty_agenda(&self, calendar: &CalendarLayout, mouse: MouseEvent) -> bool {
        let agenda_inner = calendar.agenda_area.inner(Margin {
            vertical: 1,
            horizontal: 1,
        });

        self.calendar_selected_date.is_some()
            && rect_contains(agenda_inner, mouse.column, mouse.row)
            && mouse.row.saturating_sub(agenda_inner.y) as usize >= calendar.agenda_lines.len()
    }

    async fn handle_editor_mouse(&mut self, mouse: MouseEvent, area: Rect) -> Result<(), String> {
        if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            return Ok(());
        }

        let layout = editor_layout(area);
        if rect_contains(layout.save, mouse.column, mouse.row) {
            return self.save_editor().await;
        }
        if rect_contains(layout.cancel, mouse.column, mouse.row) {
            self.editor = None;
            return Ok(());
        }

        if let Some(editor) = self.editor.as_mut() {
            if rect_contains(layout.prev, mouse.column, mouse.row) {
                editor_move_prev(editor);
            } else if rect_contains(layout.next, mouse.column, mouse.row) {
                editor_move_next(editor);
            }
        }

        Ok(())
    }

    fn kanban_columns(&self) -> Vec<KanbanColumn> {
        let Some(list) = self.selected_list() else {
            return vec![
                KanbanColumn {
                    name: tr("tui-kanban-open"),
                    is_done: false,
                },
                KanbanColumn {
                    name: tr("tui-kanban-done"),
                    is_done: true,
                },
            ];
        };

        let mut columns = list_states_to_columns(list.states.as_deref());
        if columns.is_empty() {
            columns = parse_state_config_columns(list.state_config.as_deref());
        }

        if columns.is_empty() {
            columns.push(KanbanColumn {
                name: tr("tui-kanban-open"),
                is_done: false,
            });
            columns.push(KanbanColumn {
                name: tr("tui-kanban-in-progress"),
                is_done: false,
            });
            columns.push(KanbanColumn {
                name: tr("tui-kanban-done"),
                is_done: true,
            });
        }

        columns
    }

    fn kanban_buckets(&self) -> (Vec<KanbanColumn>, Vec<Vec<usize>>) {
        let columns = self.kanban_columns();
        let mut buckets = vec![Vec::new(); columns.len()];

        let fallback_column = columns
            .iter()
            .position(|column| !column.is_done)
            .unwrap_or(0);
        let done_column = columns
            .iter()
            .position(|column| column.is_done)
            .unwrap_or_else(|| columns.len().saturating_sub(1));

        let visible_indices = self.visible_item_indices();
        for idx in visible_indices {
            let item = &self.items[idx];
            let target = if item.is_done.unwrap_or(false) {
                done_column
            } else if let Some(progress) = item.progress.as_ref() {
                columns
                    .iter()
                    .position(|column| column.name.eq_ignore_ascii_case(progress.trim()))
                    .unwrap_or(fallback_column)
            } else {
                fallback_column
            };
            buckets[target].push(idx);
        }

        (columns, buckets)
    }

    fn move_kanban_selection(&mut self, delta: isize) -> bool {
        if delta == 0 {
            return false;
        }
        let (_, buckets) = self.kanban_buckets();
        let Some(next_item) = stepped_kanban_selection(&buckets, self.selected_item, delta) else {
            return false;
        };
        if next_item == self.selected_item {
            return false;
        }
        self.selected_item = next_item;
        true
    }

    fn move_kanban_selection_wrapped(&mut self, delta: isize) -> bool {
        if delta == 0 {
            return false;
        }
        let (_, buckets) = self.kanban_buckets();
        let Some(next_item) = stepped_kanban_selection_wrapped(&buckets, self.selected_item, delta)
        else {
            return false;
        };
        if next_item == self.selected_item {
            return false;
        }
        self.selected_item = next_item;
        true
    }

    fn default_progress_value(&self) -> String {
        let columns = self.kanban_columns();
        if let Some(value) = columns
            .iter()
            .find(|column| !column.is_done)
            .map(|column| column.name.trim())
            .filter(|name| !name.is_empty())
        {
            return value.to_string();
        }
        if let Some(value) = columns
            .iter()
            .map(|column| column.name.trim())
            .find(|name| !name.is_empty())
        {
            return value.to_string();
        }

        tr("tui-kanban-open")
    }

    fn progress_suggestions(&self) -> Vec<String> {
        self.progress_choices()
    }

    fn progress_choices(&self) -> Vec<String> {
        let mut out = Vec::new();

        for column in self.kanban_columns() {
            let name = column.name.trim();
            if !name.is_empty() {
                push_unique_case_insensitive(&mut out, name);
            }
        }

        if out.is_empty() {
            out.push(tr("tui-kanban-open"));
            out.push(tr("tui-kanban-in-progress"));
            out.push(tr("tui-kanban-done"));
        }

        out
    }

    fn normalize_progress_input(&self, raw: &str) -> Option<String> {
        let value = raw.trim();
        if value.is_empty() {
            return Some(String::default());
        }

        self.progress_choices()
            .into_iter()
            .find(|choice| choice.trim().eq_ignore_ascii_case(value))
    }

    fn tag_suggestions(&self) -> Vec<String> {
        let mut out = Vec::new();
        for item in &self.items {
            if let Some(tags) = item.tags.as_ref() {
                for tag in tags {
                    let tag = tag.trim();
                    if !tag.is_empty() {
                        push_unique_case_insensitive(&mut out, tag);
                    }
                }
            }
        }

        out.sort_by_key(|value| value.to_ascii_lowercase());
        out
    }

    fn apply_editor_suggestion(&mut self, delta: isize) -> bool {
        if delta == 0 {
            return false;
        }

        let Some(active_field) = self.editor.as_ref().map(|editor| editor.active_field) else {
            return false;
        };

        match active_field {
            EditorField::Reminder => {
                let suggestions = vec![tr("label-off"), tr("label-on")];
                let current = self
                    .editor
                    .as_ref()
                    .map(|editor| editor.reminder.clone())
                    .unwrap_or_default();
                let Some(next) = cycle_suggestion_value(&current, &suggestions, delta) else {
                    return false;
                };
                if let Some(editor) = self.editor.as_mut() {
                    editor.reminder = next;
                    return true;
                }
                false
            }
            EditorField::Progress => {
                let suggestions = self.progress_suggestions();
                let current = self
                    .editor
                    .as_ref()
                    .map(|editor| editor.progress.clone())
                    .unwrap_or_default();
                let Some(next) = cycle_suggestion_value(&current, &suggestions, delta) else {
                    return false;
                };
                if let Some(editor) = self.editor.as_mut() {
                    editor.progress = next;
                    return true;
                }
                false
            }
            EditorField::Tags => {
                let suggestions = self.tag_suggestions();
                let current = self
                    .editor
                    .as_ref()
                    .map(|editor| editor.tags.clone())
                    .unwrap_or_default();
                let Some(next) = autocomplete_last_tag(&current, &suggestions, delta) else {
                    return false;
                };
                if let Some(editor) = self.editor.as_mut() {
                    editor.tags = next;
                    return true;
                }
                false
            }
            _ => false,
        }
    }

    fn move_kanban_column_selection(&mut self, delta: isize) -> bool {
        if delta == 0 {
            return false;
        }
        let (_, buckets) = self.kanban_buckets();
        let Some(next_item) = next_kanban_column_selection(&buckets, self.selected_item, delta)
        else {
            return false;
        };
        if next_item == self.selected_item {
            return false;
        }
        self.selected_item = next_item;
        true
    }

    fn register_item_click(&mut self, item_index: usize) -> bool {
        let now = Instant::now();
        if let Some((mode, last_item, at)) = self.last_item_click {
            if mode == self.mode
                && last_item == item_index
                && now.duration_since(at) <= Duration::from_millis(360)
            {
                self.last_item_click = None;
                return true;
            }
        }
        self.last_item_click = Some((self.mode, item_index, now));
        false
    }

    async fn move_item_to_kanban_column(
        &mut self,
        item_index: usize,
        target_column: usize,
    ) -> Result<(), String> {
        let (columns, _) = self.kanban_buckets();
        let Some(column) = columns.get(target_column) else {
            return Ok(());
        };
        let Some(item) = self.items.get(item_index).cloned() else {
            return Ok(());
        };

        let progress_name = column.name.trim().to_string();
        if item.progress.as_deref().map_or("", str::trim) != progress_name {
            let mut body = Map::new();
            body.insert("progress".to_string(), Value::String(progress_name));
            let updated: ListItem = self
                .api
                .put(&format!("/items/{}", item.id), &Value::Object(body))
                .await?;
            if let Some(slot) = self.items.get_mut(item_index) {
                *slot = updated;
            }
        }

        let current_done = self
            .items
            .get(item_index)
            .and_then(|it| it.is_done)
            .unwrap_or(false);
        if current_done != column.is_done {
            let mut body = Map::new();
            body.insert("done".to_string(), Value::Bool(column.is_done));
            let updated: ListItem = self
                .api
                .patch_json(&format!("/items/{}/done", item.id), &Value::Object(body))
                .await?;
            if let Some(slot) = self.items.get_mut(item_index) {
                *slot = updated;
            }
        }

        self.selected_item = item_index;
        if let Some(list_id) = self.selected_list_id() {
            self.items_cache.insert(list_id, self.items.clone());
        }
        self.status = Some(tr("cli-item-updated"));
        if self.mode == ViewMode::List {
            self.refresh_selected_image_background();
            self.load_comments_for_selected_item();
        }
        Ok(())
    }

    async fn move_item_to_calendar_date(
        &mut self,
        item_index: usize,
        target_date: SimpleDate,
    ) -> Result<(), String> {
        let Some(item) = self.items.get(item_index).cloned() else {
            return Ok(());
        };
        if item.due_date.as_deref().and_then(parse_iso_date) == Some(target_date) {
            return Ok(());
        }

        let due_date = due_date_with_preserved_time(item.due_date.as_deref(), target_date);
        self.update_item_due_date(item_index, due_date).await
    }

    async fn move_selected_item_calendar_hours(
        &mut self,
        delta_hours: i32,
    ) -> Result<bool, String> {
        if delta_hours == 0 || self.mode != ViewMode::Calendar {
            return Ok(false);
        }
        let Some(item) = self.items.get(self.selected_item).cloned() else {
            return Ok(false);
        };

        let fallback_date = self
            .calendar_selected_date
            .or_else(|| item.due_date.as_deref().and_then(parse_iso_date))
            .unwrap_or_else(today_utc);
        let due_date =
            due_date_with_hour_delta(item.due_date.as_deref(), fallback_date, delta_hours);
        if item.due_date.as_deref() == Some(due_date.as_str()) {
            return Ok(false);
        }

        self.update_item_due_date(self.selected_item, due_date)
            .await?;
        Ok(true)
    }

    async fn update_item_due_date(
        &mut self,
        item_index: usize,
        due_date: String,
    ) -> Result<(), String> {
        let Some(item) = self.items.get(item_index).cloned() else {
            return Ok(());
        };
        if item.due_date.as_deref() == Some(due_date.as_str()) {
            return Ok(());
        }

        let span = crate::telemetry::TraceSpan::child("tui.calendar", "due_date_update");
        span.set_tag("operation", "due_date_update");
        span.set_tag("mode", "calendar");
        let mut body = Map::new();
        body.insert("due_date".to_string(), Value::String(due_date.clone()));
        let updated_result: Result<ListItem, String> = self
            .api
            .put(&format!("/items/{}", item.id), &Value::Object(body))
            .await;
        span.set_status(updated_result.is_ok());
        span.finish();
        let updated = updated_result?;

        if let Some(slot) = self.items.get_mut(item_index) {
            *slot = updated;
        }
        self.selected_item = item_index;
        if let Some(target_date) = parse_iso_date(&due_date) {
            self.calendar_selected_date = Some(target_date);
            self.calendar_visible_month = Some(start_of_month(target_date));
        }
        if let Some(list_id) = self.selected_list_id() {
            self.items_cache.insert(list_id, self.items.clone());
        }
        self.status = Some(tr("cli-item-updated"));
        Ok(())
    }

    fn calendar_layout(&self, area: Rect) -> CalendarLayout {
        let inner = area.inner(Margin {
            vertical: 1,
            horizontal: 1,
        });
        let (month_area, agenda_area) = calendar_panel_layout(inner);
        let visible_indices = self.visible_item_indices();
        let month = self.calendar_month(&visible_indices);
        let mut dated: BTreeMap<SimpleDate, Vec<usize>> = BTreeMap::new();
        let mut undated = Vec::new();

        for idx in visible_indices {
            let item = &self.items[idx];
            if let Some(date) = item.due_date.as_deref().and_then(parse_iso_date) {
                if same_calendar_month(date, month) {
                    dated.entry(date).or_default().push(idx);
                }
            } else {
                undated.push(idx);
            }
        }

        let mut month_lines = Vec::new();
        let mut agenda_lines = Vec::new();
        let mut date_hits = Vec::new();
        let mut item_hits = Vec::new();
        let title = format!(
            "{}: {}",
            tr("view-calendar"),
            self.selected_list_display_name()
        );

        if inner.height == 0 || inner.width == 0 {
            return CalendarLayout {
                title,
                month_title: format!("{:04}-{:02}", month.year, month.month),
                agenda_title: tr("label-items"),
                month_area,
                agenda_area,
                month_lines,
                agenda_lines,
                date_hits,
                item_hits,
            };
        }

        let selected_date_in_month = self
            .calendar_selected_date
            .filter(|date| same_calendar_month(*date, month));
        let drag_target_date = self
            .calendar_drag_target_date
            .filter(|_| self.calendar_drag_started);

        let month_inner = month_area.inner(Margin {
            vertical: 1,
            horizontal: 1,
        });
        if month_inner.width > 0 && month_inner.height > 0 {
            let widths = calendar_cell_widths(month_inner.width.saturating_sub(6));
            let today = today_utc();
            month_lines.push(Line::styled(
                calendar_row(["Mo", "Di", "Mi", "Do", "Fr", "Sa", "So"], &widths),
                Style::default().add_modifier(Modifier::BOLD),
            ));

            let first_day = start_of_month(month);
            let first_weekday = weekday_monday0(first_day.year, first_day.month, first_day.day);
            let grid_start = shifted_date(first_day, -(first_weekday as i64));
            let week_rows = month_inner.height.saturating_sub(1).min(6) as usize;

            for week in 0..week_rows {
                let mut day_cells = Vec::with_capacity(7);
                let day_row_y = month_inner.y.saturating_add(month_lines.len() as u16);
                let mut x = month_inner.x;

                for (weekday, width) in widths.iter().enumerate() {
                    let date = shifted_date(grid_start, (week * 7 + weekday) as i64);
                    let in_month = same_calendar_month(date, month);
                    let indexes: &[usize] = if in_month {
                        dated.get(&date).map_or(&[][..], Vec::as_slice)
                    } else {
                        &[]
                    };
                    let selected_here = calendar_day_is_selected(
                        date,
                        selected_date_in_month,
                        indexes.contains(&self.selected_item),
                        drag_target_date,
                    );

                    day_cells.push(calendar_day_cell_label(
                        date,
                        in_month,
                        indexes.len(),
                        selected_here,
                        date == today,
                    ));

                    if in_month {
                        date_hits.push(CalendarDateHit {
                            rect: Rect::new(x, day_row_y, *width, 1),
                            date,
                        });
                        if let Some(item_index) = calendar_hit_item(indexes, self.selected_item) {
                            item_hits.push(CalendarItemHit {
                                rect: Rect::new(x, day_row_y, *width, 1),
                                item_index,
                            });
                        }
                    }

                    x = x.saturating_add(width.saturating_add(1));
                }

                month_lines.push(Line::from(calendar_row(day_cells, &widths)));
            }
        }

        if month_lines.is_empty() {
            month_lines.push(Line::from(tr("output-no-items")));
        }

        let agenda_inner = agenda_area.inner(Margin {
            vertical: 1,
            horizontal: 1,
        });
        if agenda_inner.width > 0 && agenda_inner.height > 0 {
            let agenda =
                self.calendar_agenda_entries(&dated, &undated, month, agenda_inner.height as usize);
            for (row, (text, item_index)) in agenda.into_iter().enumerate() {
                let y = agenda_inner.y.saturating_add(row as u16);
                agenda_lines.push(Line::from(fit_cell(&text, agenda_inner.width as usize)));
                if let Some(item_index) = item_index {
                    item_hits.push(CalendarItemHit {
                        rect: Rect::new(agenda_inner.x, y, agenda_inner.width, 1),
                        item_index,
                    });
                }
            }
        }

        if agenda_lines.is_empty() {
            agenda_lines.push(Line::from(tr("output-no-items")));
        }

        let month_title = format!("{:04}-{:02}", month.year, month.month);
        let agenda_title = selected_date_in_month.map_or_else(
            || format!("{} {:04}-{:02}", tr("label-items"), month.year, month.month),
            |date| format!("{} {}", tr("label-items"), format_iso_date(date)),
        );

        CalendarLayout {
            title,
            month_title,
            agenda_title,
            month_area,
            agenda_area,
            month_lines,
            agenda_lines,
            date_hits,
            item_hits,
        }
    }

    fn calendar_agenda_entries(
        &self,
        dated: &BTreeMap<SimpleDate, Vec<usize>>,
        undated: &[usize],
        month: SimpleDate,
        max_rows: usize,
    ) -> Vec<(String, Option<usize>)> {
        if max_rows == 0 {
            return Vec::new();
        }

        let selected_date = self
            .calendar_selected_date
            .filter(|date| same_calendar_month(*date, month));

        let mut out = Vec::new();
        match selected_date {
            Some(date) => {
                out.push((
                    tr_args(
                        "tui-calendar-selected-date",
                        &[("date", format_iso_date(date))],
                    ),
                    None,
                ));
                if let Some(indexes) = dated.get(&date) {
                    push_calendar_agenda_items(
                        &mut out,
                        indexes,
                        &self.items,
                        self.selected_item,
                        max_rows,
                    );
                }
            }
            None => push_calendar_month_agenda_entries(
                &mut out,
                dated,
                undated,
                &self.items,
                self.selected_item,
                max_rows,
            ),
        }

        if out.is_empty() {
            out.push((tr("output-no-items"), None));
        }
        out.truncate(max_rows);
        out
    }

    fn calendar_month(&self, visible_indices: &[usize]) -> SimpleDate {
        if let Some(date) = self.calendar_visible_month {
            return start_of_month(date);
        }

        if let Some(date) = self.calendar_selected_date {
            return start_of_month(date);
        }

        if let Some(date) = self
            .selected_item()
            .and_then(|item| item.due_date.as_deref())
            .and_then(parse_iso_date)
        {
            return start_of_month(date);
        }

        for idx in visible_indices {
            if let Some(date) = self.items[*idx]
                .due_date
                .as_deref()
                .and_then(parse_iso_date)
            {
                return start_of_month(date);
            }
        }

        let today = today_utc();
        start_of_month(today)
    }
}

fn calendar_panel_layout(area: Rect) -> (Rect, Rect) {
    if area.width >= 72 && area.height >= 10 {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(area);
        return (chunks[0], chunks[1]);
    }

    let month_height = area.height.saturating_sub(5).clamp(4, 9);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(month_height), Constraint::Min(3)])
        .split(area);
    (chunks[0], chunks[1])
}

fn calendar_day_cell_label(
    date: SimpleDate,
    in_month: bool,
    item_count: usize,
    selected: bool,
    is_today: bool,
) -> String {
    if !in_month {
        return format!("({:02})", date.day);
    }

    let marker = match item_count {
        0 => ' ',
        1..=9 => char::from_digit(item_count as u32, 10).unwrap_or('+'),
        _ => '+',
    };

    if selected {
        format!("[{:02}{marker}]", date.day)
    } else if is_today {
        format!("*{:02}{marker}", date.day)
    } else {
        format!(" {:02}{marker}", date.day)
    }
}

fn parse_iso_date(raw: &str) -> Option<SimpleDate> {
    let date = raw.get(0..10)?;
    let mut parts = date.split('-');
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) {
        return None;
    }
    if day == 0 || day > days_in_month(year, month) {
        return None;
    }
    Some(SimpleDate { year, month, day })
}

fn valid_due_date_input(raw: &str) -> bool {
    let value = raw.trim();
    if value.is_empty() {
        return true;
    }
    if parse_iso_date(value).is_none() {
        return false;
    }
    if value.len() == 10 {
        return true;
    }
    if parse_due_time(value).is_none() {
        return false;
    }
    value
        .get(16..)
        .unwrap_or_default()
        .chars()
        .all(|ch| ch.is_ascii_digit() || matches!(ch, ':' | 'Z' | 'z' | '+' | '-'))
}

fn due_date_input_prefix_allowed(raw: &str) -> bool {
    let value = raw.trim();
    value.len() <= 25
        && value.chars().all(|ch| {
            ch.is_ascii_digit() || matches!(ch, '-' | 'T' | 't' | ':' | 'Z' | 'z' | '+' | ' ')
        })
}

fn due_date_time_suffix(raw: &str) -> Option<&str> {
    if raw.len() <= 10 {
        return None;
    }
    let suffix = raw.get(10..)?;
    if suffix.starts_with('T') || suffix.starts_with(' ') {
        Some(suffix)
    } else {
        None
    }
}

fn due_date_with_preserved_time(
    existing_due_date: Option<&str>,
    target_date: SimpleDate,
) -> String {
    let base = format_iso_date(target_date);
    if let Some(suffix) = existing_due_date.and_then(due_date_time_suffix) {
        format!("{base}{suffix}")
    } else {
        base
    }
}

fn parse_due_time(raw: &str) -> Option<(u32, u32)> {
    let bytes = raw.as_bytes();
    if bytes.get(10).copied()? != b'T' && bytes.get(10).copied()? != b' ' {
        return None;
    }
    if bytes.get(13).copied()? != b':' {
        return None;
    }
    let hour = raw.get(11..13)?.parse::<u32>().ok()?;
    let minute = raw.get(14..16)?.parse::<u32>().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    Some((hour, minute))
}

fn due_date_with_hour_delta(
    existing_due_date: Option<&str>,
    fallback_date: SimpleDate,
    delta_hours: i32,
) -> String {
    let mut date = existing_due_date
        .and_then(parse_iso_date)
        .unwrap_or(fallback_date);
    let (hour, minute) = existing_due_date.and_then(parse_due_time).unwrap_or((9, 0));
    let mut next_hour = hour as i32 + delta_hours;

    while next_hour < 0 {
        date = shifted_date(date, -1);
        next_hour += 24;
    }
    while next_hour >= 24 {
        date = shifted_date(date, 1);
        next_hour -= 24;
    }

    format!("{}T{:02}:{:02}", format_iso_date(date), next_hour, minute)
}

fn format_iso_date(date: SimpleDate) -> String {
    format!("{:04}-{:02}-{:02}", date.year, date.month, date.day)
}

fn start_of_month(date: SimpleDate) -> SimpleDate {
    SimpleDate {
        year: date.year,
        month: date.month,
        day: 1,
    }
}

fn shifted_date(date: SimpleDate, delta_days: i64) -> SimpleDate {
    civil_from_days(days_from_civil(date.year, date.month, date.day) + delta_days)
}

fn shifted_month(date: SimpleDate, delta_months: i32) -> SimpleDate {
    let zero_based_month = date.month as i32 - 1;
    let total_months = date
        .year
        .saturating_mul(12)
        .saturating_add(zero_based_month)
        .saturating_add(delta_months);
    let year = total_months.div_euclid(12);
    let month = total_months.rem_euclid(12) as u32 + 1;
    let day = date.day.min(days_in_month(year, month));
    SimpleDate { year, month, day }
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 30,
    }
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn weekday_monday0(year: i32, month: u32, day: u32) -> u32 {
    (days_from_civil(year, month, day) + 3).rem_euclid(7) as u32
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = year as i64 - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month as i64;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(days: i64) -> SimpleDate {
    let days = days + 719468;
    let era = if days >= 0 { days } else { days - 146096 } / 146097;
    let doe = days - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    SimpleDate {
        year: (year + i64::from(month <= 2)) as i32,
        month: month as u32,
        day: day as u32,
    }
}

fn today_utc() -> SimpleDate {
    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| (duration.as_secs() / 86_400) as i64);
    civil_from_days(days)
}

fn calendar_cell_widths(width: u16) -> [u16; 7] {
    if width == 0 {
        return [0; 7];
    }
    let base = width / 7;
    let mut widths = [base; 7];
    let used = base.saturating_mul(7);
    let mut rest = width.saturating_sub(used);
    let mut index = 0;
    while rest > 0 && index < widths.len() {
        widths[index] = widths[index].saturating_add(1);
        rest -= 1;
        index += 1;
    }
    widths
}

fn calendar_row<I, S>(cells: I, widths: &[u16; 7]) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    cells
        .into_iter()
        .zip(widths.iter())
        .map(|(cell, width)| fit_cell(cell.as_ref(), *width as usize))
        .collect::<Vec<_>>()
        .join("│")
}

fn fit_cell(value: &str, width: usize) -> String {
    if width == 0 {
        return String::default();
    }
    let mut text: String = value.chars().take(width).collect();
    let len = text.chars().count();
    if len < width {
        text.push_str(&" ".repeat(width - len));
    }
    text
}

fn wrap_plain_row(first_prefix: &str, next_prefix: &str, value: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![format!("{first_prefix}{value}")];
    }

    let next_width = next_prefix.chars().count();
    if width <= next_width.saturating_add(4) {
        return vec![format!("{first_prefix}{value}")];
    }

    let mut lines = Vec::new();
    let mut line = first_prefix.to_string();
    let mut col = first_prefix.chars().count();
    for ch in value.chars() {
        let ch_width = 1;
        if col + ch_width > width && col > next_width {
            lines.push(line);
            line = next_prefix.to_string();
            col = next_width;
            if ch == ' ' {
                continue;
            }
        }
        line.push(ch);
        col += ch_width;
    }
    lines.push(line);
    lines
}

fn wrapped_item_row_height(item: &ListItem, width: usize) -> usize {
    wrap_plain_row("  ", "  ", &item_row_text(item), width).len()
}

fn visible_item_at_wrapped_row(
    items: &[ListItem],
    visible: &[usize],
    start: usize,
    target_row: usize,
    width: usize,
) -> Option<usize> {
    let mut row = 0;
    for item_index in visible.iter().copied().skip(start) {
        let item = items.get(item_index)?;
        let height = wrapped_item_row_height(item, width);
        if target_row < row + height {
            return Some(item_index);
        }
        row += height;
    }
    None
}

fn calendar_hit_item(indexes: &[usize], selected_item: usize) -> Option<usize> {
    indexes
        .iter()
        .copied()
        .find(|idx| *idx == selected_item)
        .or_else(|| indexes.first().copied())
}

fn same_calendar_month(date: SimpleDate, month: SimpleDate) -> bool {
    date.year == month.year && date.month == month.month
}

fn calendar_day_is_selected(
    date: SimpleDate,
    selected_date: Option<SimpleDate>,
    selected_item_on_day: bool,
    drag_target_date: Option<SimpleDate>,
) -> bool {
    drag_target_date == Some(date)
        || selected_date == Some(date)
        || (selected_date.is_none() && selected_item_on_day)
}

fn calendar_date_at(layout: &CalendarLayout, column: u16, row: u16) -> Option<SimpleDate> {
    layout
        .date_hits
        .iter()
        .find(|hit| rect_contains(hit.rect, column, row))
        .map(|hit| hit.date)
}

fn calendar_pointer_date(
    area: Rect,
    column: u16,
    row: u16,
    month: SimpleDate,
) -> Option<SimpleDate> {
    let inner = area.inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    if inner.width == 0 || inner.height == 0 {
        return None;
    }

    let (month_area, _) = calendar_panel_layout(inner);
    let month_inner = month_area.inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    if month_inner.width == 0 || month_inner.height == 0 {
        return None;
    }

    let day_rows_start = month_inner.y.saturating_add(1);
    let week_rows = month_inner.height.saturating_sub(1).min(6);
    if week_rows == 0 || row < day_rows_start || row >= day_rows_start.saturating_add(week_rows) {
        return None;
    }

    let widths = calendar_cell_widths(month_inner.width.saturating_sub(6));
    let mut x = month_inner.x;
    let mut weekday_index = None;
    for (idx, width) in widths.iter().enumerate() {
        if *width > 0 && column >= x && column < x.saturating_add(*width) {
            weekday_index = Some(idx);
            break;
        }

        x = x.saturating_add(*width);
        if idx + 1 < widths.len() {
            if column == x {
                return None;
            }
            x = x.saturating_add(1);
        }
    }

    let weekday_index = weekday_index?;
    let week_index = row.saturating_sub(day_rows_start) as usize;
    let first_day = start_of_month(month);
    let first_weekday = weekday_monday0(first_day.year, first_day.month, first_day.day);
    let grid_start = shifted_date(first_day, -(first_weekday as i64));
    let date = shifted_date(grid_start, (week_index * 7 + weekday_index) as i64);
    same_calendar_month(date, month).then_some(date)
}

fn calendar_item_at(layout: &CalendarLayout, column: u16, row: u16) -> Option<usize> {
    layout
        .item_hits
        .iter()
        .find(|hit| rect_contains(hit.rect, column, row))
        .map(|hit| hit.item_index)
}

fn push_calendar_agenda_items(
    out: &mut Vec<(String, Option<usize>)>,
    indexes: &[usize],
    items: &[ListItem],
    selected_item: usize,
    max_rows: usize,
) {
    let available = max_rows.saturating_sub(out.len());
    if available == 0 {
        return;
    }
    let visible_items = available.saturating_sub(1).clamp(1, indexes.len());
    for item_index in indexes.iter().copied().take(visible_items) {
        let marker = if item_index == selected_item {
            ">"
        } else {
            " "
        };
        out.push((
            format!("{marker} {}", item_short_text(&items[item_index])),
            Some(item_index),
        ));
    }
    let remaining = indexes.len().saturating_sub(visible_items);
    if remaining > 0 && out.len() < max_rows {
        out.push((
            tr_args("tui-calendar-more", &[("count", remaining.to_string())]),
            None,
        ));
    }
}

fn push_calendar_month_agenda_entries(
    out: &mut Vec<(String, Option<usize>)>,
    dated: &BTreeMap<SimpleDate, Vec<usize>>,
    undated: &[usize],
    items: &[ListItem],
    selected_item: usize,
    max_rows: usize,
) {
    if max_rows == 0 {
        return;
    }

    for (date, indexes) in dated {
        if out.len() >= max_rows {
            return;
        }
        out.push((format_iso_date(*date), None));
        push_calendar_agenda_items(out, indexes, items, selected_item, max_rows);
    }

    if !undated.is_empty() && out.len() < max_rows {
        out.push((
            tr_args(
                "tui-calendar-undated",
                &[("count", undated.len().to_string())],
            ),
            None,
        ));
        push_calendar_agenda_items(out, undated, items, selected_item, max_rows);
    }
}

fn active_editor_value_mut(editor: &mut EditorState) -> &mut String {
    match editor.active_field {
        EditorField::Text => &mut editor.text,
        EditorField::Quantity => &mut editor.quantity,
        EditorField::DueDate => &mut editor.due_date,
        EditorField::DueTime => &mut editor.due_time,
        EditorField::PlannedDate => &mut editor.planned_date,
        EditorField::PlannedTime => &mut editor.planned_time,
        EditorField::Reminder => &mut editor.reminder,
        EditorField::ReminderTime => &mut editor.reminder_time,
        EditorField::ReminderOffsets => &mut editor.reminder_offsets,
        EditorField::TravelTimeMinutes => &mut editor.travel_time_minutes,
        EditorField::Priority => &mut editor.priority,
        EditorField::Tags => &mut editor.tags,
        EditorField::Progress => &mut editor.progress,
        EditorField::Notes => &mut editor.notes,
    }
}

fn active_editor_value(editor: &EditorState) -> &String {
    match editor.active_field {
        EditorField::Text => &editor.text,
        EditorField::Quantity => &editor.quantity,
        EditorField::DueDate => &editor.due_date,
        EditorField::DueTime => &editor.due_time,
        EditorField::PlannedDate => &editor.planned_date,
        EditorField::PlannedTime => &editor.planned_time,
        EditorField::Reminder => &editor.reminder,
        EditorField::ReminderTime => &editor.reminder_time,
        EditorField::ReminderOffsets => &editor.reminder_offsets,
        EditorField::TravelTimeMinutes => &editor.travel_time_minutes,
        EditorField::Priority => &editor.priority,
        EditorField::Tags => &editor.tags,
        EditorField::Progress => &editor.progress,
        EditorField::Notes => &editor.notes,
    }
}

fn editor_bool_label(value: Option<bool>) -> String {
    match value {
        Some(true) => tr("label-on"),
        Some(false) => tr("label-off"),
        None => String::default(),
    }
}

fn parse_editor_bool_input(raw: &str) -> Option<bool> {
    let value = raw.trim();
    if value.is_empty() {
        return None;
    }
    let normalized = value.to_ascii_lowercase();
    match normalized.as_str() {
        "1" | "true" | "yes" | "y" | "on" => return Some(true),
        "0" | "false" | "no" | "n" | "off" => return Some(false),
        _ => {}
    }

    let localized = value.to_lowercase();
    if localized == tr("label-on").to_lowercase() {
        return Some(true);
    }
    if localized == tr("label-off").to_lowercase() {
        return Some(false);
    }
    None
}

fn tags_value(raw: &str) -> Value {
    Value::Array(
        raw.split(',')
            .map(str::trim)
            .filter(|tag| !tag.is_empty())
            .map(|tag| Value::String(tag.to_string()))
            .collect(),
    )
}

fn parse_i64_csv(raw: &str) -> Vec<i64> {
    raw.split(',')
        .filter_map(|part| part.trim().parse::<i64>().ok())
        .collect()
}

fn editor_reminder_details_provided(reminder_time: &str, reminder_offsets: &[i64]) -> bool {
    !reminder_time.trim().is_empty() || !reminder_offsets.is_empty()
}

fn invite_url_from_response(resp: &Value) -> Option<String> {
    resp.get("invite_url")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            resp.get("invite_token")
                .and_then(Value::as_str)
                .map(|token| format!("https://kram.li/i/{token}"))
        })
        .or_else(|| {
            resp.get("token")
                .and_then(Value::as_str)
                .map(|token| format!("https://kram.li/i/{token}"))
        })
}

fn default_handoff_device_label() -> String {
    std::env::var(KRAMLI_DEVICE_LABEL_ENV)
        .ok()
        .map(|value| value.trim().chars().take(80).collect::<String>())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "Kramli CLI".to_string())
}

fn should_send_auto_handoff(
    selected_list_id: Option<i64>,
    pending_list_id: Option<i64>,
    due_list_id: i64,
) -> bool {
    selected_list_id == Some(due_list_id) && pending_list_id == Some(due_list_id)
}

fn auto_handoff_enabled() -> bool {
    auto_handoff_enabled_from_value(std::env::var(KRAMLI_AUTO_HANDOFF_ENV).ok().as_deref())
}

fn auto_handoff_enabled_from_value(raw: Option<&str>) -> bool {
    raw.and_then(|value| match value.trim().to_ascii_lowercase().as_str() {
        "0" | "false" | "off" | "no" => Some(false),
        "1" | "true" | "on" | "yes" => Some(true),
        _ => None,
    })
    .unwrap_or(true)
}

fn editor_fields(mode: EditorMode) -> &'static [EditorField] {
    match mode {
        EditorMode::Create | EditorMode::Edit => &ITEM_EDITOR_FIELDS,
        EditorMode::Comment | EditorMode::Filter => &SIMPLE_EDITOR_FIELDS,
    }
}

fn editor_step_index(editor: &EditorState) -> usize {
    editor_fields(editor.mode)
        .iter()
        .position(|field| *field == editor.active_field)
        .unwrap_or(0)
}

fn editor_move_next(editor: &mut EditorState) {
    let fields = editor_fields(editor.mode);
    let index = editor_step_index(editor);
    if let Some(next) = fields.get(index.saturating_add(1)) {
        editor.active_field = *next;
    }
}

fn editor_move_prev(editor: &mut EditorState) {
    let fields = editor_fields(editor.mode);
    let index = editor_step_index(editor);
    if index > 0 {
        editor.active_field = fields[index - 1];
    }
}

fn editor_field_hint(field: EditorField, mode: EditorMode) -> String {
    match mode {
        EditorMode::Filter => tr("label-text"),
        EditorMode::Comment => tr("label-comments"),
        EditorMode::Create | EditorMode::Edit => field.label(),
    }
}

fn ui_layout(area: Rect) -> UiLayout {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(6),
            Constraint::Length(5),
        ])
        .split(area);

    let tab_chunks_vec = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(33),
            Constraint::Percentage(34),
            Constraint::Percentage(33),
        ])
        .split(vertical[0]);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(vertical[1]);

    UiLayout {
        lists: body[0],
        content: body[1],
        footer: vertical[2],
        tab_chunks: [tab_chunks_vec[0], tab_chunks_vec[1], tab_chunks_vec[2]],
    }
}

fn list_mode_layout(area: Rect) -> (Rect, Rect) {
    if area.width >= 90 && area.height >= 18 {
        let split = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
            .split(area);
        return (split[0], split[1]);
    }

    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(area);
    (split[0], split[1])
}

fn kanban_chunks(area: Rect, count: usize) -> Vec<Rect> {
    if count == 0 {
        return Vec::new();
    }
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints(vec![Constraint::Ratio(1, count as u32); count])
        .split(area)
        .to_vec()
}

fn kanban_visible_range(
    area: Rect,
    total_columns: usize,
    selected_column: usize,
) -> (usize, usize) {
    const MIN_COL_WIDTH: u16 = 28;
    if total_columns == 0 {
        return (0, 0);
    }

    let max_cols = ((area.width.saturating_sub(2)) / MIN_COL_WIDTH).max(1) as usize;
    if total_columns <= max_cols {
        return (0, total_columns);
    }

    let mut start = selected_column.saturating_sub(max_cols / 2);
    if start + max_cols > total_columns {
        start = total_columns - max_cols;
    }
    (start, max_cols)
}

fn footer_buttons(area: Rect, key_bindings: &KeyBindings) -> Vec<(FooterAction, Rect)> {
    let content = area.inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    if content.width == 0 || content.height <= 1 {
        return Vec::new();
    }
    let actions = [
        FooterAction::Help,
        FooterAction::Add,
        FooterAction::Edit,
        FooterAction::ToggleDone,
        FooterAction::Delete,
        FooterAction::Filter,
        FooterAction::Refresh,
        FooterAction::Comment,
        FooterAction::OpenImage,
        FooterAction::Members,
        FooterAction::Invite,
        FooterAction::Undo,
        FooterAction::Quit,
    ];

    let mut out = Vec::new();
    let mut row: u16 = 0;
    let mut x = content.x;
    let button_rows = content.height.saturating_sub(1);
    for action in actions {
        let chip = action_chip_text(action, key_bindings);
        let width = (chip.chars().count() as u16).saturating_add(1);

        if width > content.width {
            continue;
        }

        if x.saturating_add(width) > content.x.saturating_add(content.width) {
            row = row.saturating_add(1);
            if row >= button_rows {
                break;
            }
            x = content.x;
        }

        let y = content.y.saturating_add(row);
        out.push((action, Rect::new(x, y, width, 1)));
        x = x.saturating_add(width.saturating_add(1));
    }
    out
}

fn editor_layout(area: Rect) -> EditorLayout {
    let (outer, inner) = centered_popup(area, 64, 96, 14, 14);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(5),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    let button_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Min(1),
        ])
        .split(rows[3]);

    EditorLayout {
        outer,
        progress: rows[0],
        field: rows[1],
        hint: rows[2],
        prev: button_row[0],
        next: button_row[1],
        save: button_row[2],
        cancel: button_row[3],
    }
}

fn beta_consent_layout(area: Rect) -> BetaConsentLayout {
    let (outer, inner) = centered_popup(area, 56, 88, 10, 14);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    let button_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(48),
            Constraint::Percentage(4),
            Constraint::Percentage(48),
        ])
        .split(rows[2]);

    BetaConsentLayout {
        outer,
        body: rows[0],
        hint: rows[1],
        accept: button_row[0],
        decline: button_row[2],
    }
}

fn centered_popup(
    area: Rect,
    min_width: u16,
    max_width: u16,
    min_height: u16,
    max_height: u16,
) -> (Rect, Rect) {
    let available_width = area.width.saturating_sub(2).max(1);
    let available_height = area.height.saturating_sub(2).max(1);
    let preferred_width = area.width.saturating_sub(8).min(max_width);
    let preferred_height = area.height.saturating_sub(4).min(max_height);
    let width = if available_width < min_width {
        available_width
    } else {
        preferred_width
            .clamp(min_width, max_width)
            .min(available_width)
    };
    let height = if available_height < min_height {
        available_height
    } else {
        preferred_height
            .clamp(min_height, max_height)
            .min(available_height)
    };
    let x = area.x.saturating_add(area.width.saturating_sub(width) / 2);
    let y = area
        .y
        .saturating_add(area.height.saturating_sub(height) / 2);
    let outer = Rect::new(x, y, width, height);
    let inner = outer.inner(Margin {
        vertical: 1,
        horizontal: 2,
    });
    (outer, inner)
}

fn draw_ui(frame: &mut Frame<'_>, app: &mut App) {
    let layout = ui_layout(frame.area());

    draw_mode_tabs(frame, app, &layout);

    draw_lists_panel(frame, app, layout.lists);

    match app.mode {
        ViewMode::List => draw_list_mode(frame, app, layout.content),
        ViewMode::Kanban => draw_kanban_mode(frame, app, layout.content),
        ViewMode::Calendar => draw_calendar_mode(frame, app, layout.content),
    }

    draw_footer(frame, app, layout.footer);

    if app.requires_beta_consent() {
        draw_beta_consent_overlay(frame, app);
        return;
    }

    if app.requires_legal_consent() {
        draw_legal_consent_overlay(frame, app);
        return;
    }

    if let Some(editor) = app.editor.as_ref() {
        draw_editor(frame, editor);
    }

    if app.show_help {
        draw_help_overlay(frame);
    }
}

fn draw_mode_tabs(frame: &mut Frame<'_>, app: &App, layout: &UiLayout) {
    for (idx, rect) in layout.tab_chunks.iter().enumerate() {
        let mode = match idx {
            0 => ViewMode::List,
            1 => ViewMode::Kanban,
            _ => ViewMode::Calendar,
        };
        let selected = app.mode == mode;
        let title = match mode {
            ViewMode::List => tr("view-list"),
            ViewMode::Kanban => tr("view-board"),
            ViewMode::Calendar => tr("view-calendar"),
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .title(if idx == 0 { "kramli" } else { "" })
            .border_style(if selected {
                Style::default().fg(ACCENT)
            } else {
                Style::default()
            });
        let widget = Paragraph::new(title)
            .alignment(Alignment::Center)
            .style(if selected {
                Style::default()
                    .fg(Color::White)
                    .bg(SELECTED_BG)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(MUTED_TEXT)
            })
            .block(block);
        frame.render_widget(widget, *rect);
    }
}

fn draw_lists_panel(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let (profile_area, list_area) = if area.height >= 9 {
        let split = list_panel_layout(area);
        (Some(split[0]), split[1])
    } else {
        (None, area)
    };

    if let Some(profile_area) = profile_area {
        let profile_block = Block::default()
            .borders(Borders::ALL)
            .title(tr("label-name"));
        frame.render_widget(profile_block, profile_area);
        let inner = profile_area.inner(Margin {
            vertical: 1,
            horizontal: 1,
        });
        let profile_text = app
            .profile_name
            .clone()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| tr("common-unknown"));
        let list_count = app.lists.len();
        let selected = app.selected_list_display_name();
        let info = vec![
            Line::from(profile_text),
            Line::from(format!("{} {}", tr("label-lists"), list_count)),
            Line::from(format!("{} {}", tr("view-list"), selected)),
        ];
        let mut info = info;
        if cfg!(debug_assertions) {
            if let Some(summary) = app.image_runtime_info.as_deref() {
                info.push(Line::from(summary.to_string()));
            }
            for line in &app.image_runtime_debug {
                info.push(Line::from(line.clone()));
            }
        }
        let text_area = if app.profile_image.is_some() && inner.width >= 14 && inner.height >= 3 {
            let image_width = inner.height.clamp(4, 8).min(inner.width.saturating_sub(8));
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Length(image_width),
                    Constraint::Length(2),
                    Constraint::Min(6),
                ])
                .split(inner);
            if let Some(profile_image) = app.profile_image.as_mut() {
                frame.render_stateful_widget(
                    StatefulImage::default().resize(Resize::Fit(Some(FilterType::Lanczos3))),
                    chunks[0],
                    &mut profile_image.protocol,
                );
            }
            chunks[2]
        } else {
            inner
        };
        frame.render_widget(
            Paragraph::new(info)
                .style(Style::default().fg(Color::Reset))
                .wrap(Wrap { trim: true }),
            text_area,
        );
    }

    let panel_rows = list_panel_rows(&app.lists);
    let selected_row = selected_list_panel_row(&panel_rows, app.selected_list);
    let visible_rows = list_area.height.saturating_sub(2) as usize;
    app.list_scroll = scroll_to_visible(app.list_scroll, selected_row, visible_rows);
    let start = app.list_scroll.min(panel_rows.len());
    let end = start.saturating_add(visible_rows).min(panel_rows.len());

    let mut icon_targets: Vec<(String, Rect)> = Vec::new();
    let mut list_items: Vec<TuiListItem> = Vec::new();
    for (row_idx, panel_row) in panel_rows.iter().enumerate().take(end).skip(start) {
        if panel_row.list_index.is_none() {
            let indent = "  ".repeat(panel_row.depth);
            let folder_asset = "folder2";
            let use_image_icon = list_icon_image_enabled(
                app.bootstrap_icons_enabled,
                app.inline_images_enabled,
                app.picker.protocol_type(),
                Some(folder_asset),
            );
            let folder_icon = if use_image_icon {
                app.ensure_list_icon_background(folder_asset);
                icon_targets.push((
                    folder_asset.to_string(),
                    Rect::new(
                        list_area
                            .x
                            .saturating_add(3)
                            .saturating_add((panel_row.depth * 2) as u16),
                        list_area.y.saturating_add(1 + (row_idx - start) as u16),
                        2,
                        1,
                    ),
                ));
                "  ".to_string()
            } else {
                bootstrap_icon_for_tui("bi-folder2", tui_icon_style())
            };
            list_items.push(TuiListItem::new(format!(
                "  {indent}{folder_icon} {}",
                panel_row.label
            )));
            continue;
        }

        let idx = panel_row.list_index.unwrap_or(0);
        let list = &app.lists[idx];
        let list_id = list.id;
        let raw_icon = list.icon.clone();
        let name = panel_row.label.clone();
        let total = list.item_count.unwrap_or(0);
        let done = list.done_count.unwrap_or(0);
        let is_archived = list.archived.unwrap_or(false);
        let row = list_area.y.saturating_add(1 + (row_idx - start) as u16);

        let icon_asset = list_icon_asset_name(raw_icon.as_deref());
        let use_image_icon = list_icon_image_enabled(
            app.bootstrap_icons_enabled,
            app.inline_images_enabled,
            app.picker.protocol_type(),
            icon_asset.as_deref(),
        );
        if let Some(asset) = icon_asset.as_deref() {
            if use_image_icon {
                app.ensure_list_icon_background(asset);
                icon_targets.push((
                    asset.to_string(),
                    Rect::new(
                        list_area
                            .x
                            .saturating_add(3)
                            .saturating_add((panel_row.depth * 2) as u16),
                        row,
                        2,
                        1,
                    ),
                ));
            }
        }

        let icon = if use_image_icon {
            "  ".to_string()
        } else {
            list_icon_for_tui(raw_icon.as_deref())
        };
        let indent = "  ".repeat(panel_row.depth);
        let trailing_base_x = list_area
            .x
            .saturating_add(7)
            .saturating_add(indent.chars().count() as u16)
            .saturating_add(name.chars().count() as u16);
        let mut trailing_cells = 0;
        let archive_marker = if is_archived {
            trailing_list_icon_marker(
                app,
                &mut icon_targets,
                ARCHIVED_LIST_ICON,
                row,
                trailing_base_x,
                &mut trailing_cells,
            )
        } else {
            String::default()
        };
        let open = (total - done).max(0);
        let marker = if idx == app.selected_list { ">" } else { " " };
        list_items.push(TuiListItem::new(format!(
            "{marker} {indent}{icon} {name}{archive_marker} #{} ({open}/{total})",
            list_id
        )));
    }

    let mut state = ListState::default();
    if !list_items.is_empty() {
        state.select(Some(selected_row.saturating_sub(start)));
    }

    let border_style = if app.focus == FocusPane::Lists {
        Style::default().fg(ACCENT)
    } else {
        Style::default()
    };

    let widget = TuiList::new(list_items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(tr("label-lists"))
                .border_style(border_style),
        )
        .highlight_style(
            Style::default()
                .bg(SELECTED_BG)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("");

    frame.render_stateful_widget(widget, list_area, &mut state);
    for (icon, rect) in icon_targets {
        if let Some(protocol) = app.list_icon_images.get(&icon) {
            frame.render_widget(TuiImage::new(protocol).allow_clipping(true), rect);
        }
    }
}

fn list_panel_layout(area: Rect) -> std::rc::Rc<[Rect]> {
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(profile_panel_height()),
            Constraint::Min(6),
        ])
        .split(area)
}

fn profile_panel_height() -> u16 {
    if cfg!(debug_assertions) {
        8
    } else {
        5
    }
}

fn list_panel_rows_area(area: Rect) -> Rect {
    if area.height >= 9 {
        list_panel_layout(area)[1]
    } else {
        area
    }
}

fn item_rows_area(area: Rect) -> Rect {
    area.inner(Margin {
        vertical: 1,
        horizontal: 1,
    })
}

fn draw_list_mode(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let (list_rect, detail_rect) = list_mode_layout(area);

    let visible_rows = list_rect.height.saturating_sub(2) as usize;
    let visible_indices = app.visible_item_indices();
    let selected_pos = app.selected_visible_position(&visible_indices);
    app.item_scroll = scroll_to_visible(app.item_scroll, selected_pos, visible_rows);
    let start = app.item_scroll.min(visible_indices.len());
    let end = start
        .saturating_add(visible_rows)
        .min(visible_indices.len());

    let row_width = list_rect.width.saturating_sub(2) as usize;
    let mut rows: Vec<TuiListItem> = visible_indices
        .iter()
        .skip(start)
        .take(end.saturating_sub(start))
        .map(|item_idx| {
            let item = &app.items[*item_idx];
            let marker = if *item_idx == app.selected_item {
                "> "
            } else {
                "  "
            };
            let lines = wrap_plain_row(marker, "  ", &item_row_text(item), row_width)
                .into_iter()
                .map(Line::from)
                .collect::<Vec<_>>();
            TuiListItem::new(Text::from(lines))
        })
        .collect();

    if rows.is_empty() {
        rows.push(TuiListItem::new(item_placeholder(app)));
    }

    let mut state = ListState::default();
    if !visible_indices.is_empty() {
        state.select(Some(selected_pos.saturating_sub(start)));
    }

    let border_style = if app.focus == FocusPane::Items {
        Style::default().fg(ACCENT)
    } else {
        Style::default()
    };

    let list_widget = TuiList::new(rows)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(
                    "{}: {}",
                    tr("label-items"),
                    app.selected_list_display_name()
                ))
                .border_style(border_style),
        )
        .highlight_style(
            Style::default()
                .bg(SELECTED_BG)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("");
    frame.render_stateful_widget(list_widget, list_rect, &mut state);

    draw_item_detail(frame, app, detail_rect);
}

fn draw_item_detail(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let item = app.selected_item().cloned();
    let image_source = item
        .as_ref()
        .and_then(App::selected_image_source)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    if image_source.is_some() && app.inline_images_enabled {
        app.refresh_selected_image_background();
    }

    let has_detail_image = image_source.as_ref().is_some_and(|source| {
        app.detail_image
            .as_ref()
            .is_some_and(|state| state.source == *source)
    });

    let detail_areas = if has_detail_image && area.height >= 12 {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length((area.height * 4) / 5),
                Constraint::Min(4),
            ])
            .split(area);
        (Some(chunks[0]), chunks[1])
    } else {
        (None, area)
    };

    if let Some(image_area) = detail_areas.0 {
        let image_block = Block::default()
            .borders(Borders::ALL)
            .title(tr("label-image"));
        frame.render_widget(image_block, image_area);
        let inner = image_area.inner(Margin {
            vertical: 1,
            horizontal: 1,
        });

        if let Some(image_state) = app.detail_image.as_mut() {
            let image_widget =
                StatefulImage::default().resize(Resize::Fit(Some(FilterType::Lanczos3)));
            frame.render_stateful_widget(image_widget, inner, &mut image_state.protocol);
        }
    }

    let mut lines: Vec<Line<'static>> = Vec::new();
    if let Some(item) = item.as_ref() {
        lines.push(Line::styled(
            item.text.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        ));
        lines.push(Line::from(format!(
            "{}: {}",
            tr("label-state").trim_end_matches(':'),
            item.progress
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .unwrap_or("-")
        )));
        lines.push(Line::from(format!(
            "{}: {}",
            tr("label-due").trim_end_matches(':'),
            date_with_time_display(item.due_date.as_deref(), item.due_time.as_deref())
        )));
        lines.push(Line::from(format!(
            "{}: {}",
            tr("label-planned").trim_end_matches(':'),
            date_with_time_display(item.planned_date.as_deref(), item.planned_time.as_deref())
        )));
        if let Some(repeat) = item
            .repeat_label
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            lines.push(Line::from(format!(
                "{}: {repeat}",
                tr("label-repeat").trim_end_matches(':')
            )));
        }
        if let Some(reminder) = item.reminder {
            let state = if reminder {
                tr("label-on")
            } else {
                tr("label-off")
            };
            lines.push(Line::from(format!(
                "{}: {state}",
                tr("label-reminder").trim_end_matches(':')
            )));
            if reminder {
                if let Some(time) = item
                    .reminder_time
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    lines.push(Line::from(format!(
                        "{}: {time}",
                        tr("label-reminder-time").trim_end_matches(':')
                    )));
                }
                if let Some(offsets) = item
                    .reminder_offsets
                    .as_ref()
                    .filter(|offsets| !offsets.is_empty())
                {
                    lines.push(Line::from(format!(
                        "{}: {}",
                        tr("label-reminder-offsets").trim_end_matches(':'),
                        reminder_offsets_display(offsets)
                    )));
                }
            }
        }
        if let Some(minutes) = item.travel_time_minutes.filter(|minutes| *minutes > 0) {
            lines.push(Line::from(format!(
                "{}: {minutes} min",
                tr("label-travel-time").trim_end_matches(':')
            )));
        }
        lines.push(Line::from(format!(
            "{}: {}",
            tr("label-quantity").trim_end_matches(':'),
            item.quantity
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .unwrap_or("-")
        )));
        lines.push(Line::from(format!(
            "{}: {}",
            "!",
            item.priority
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .unwrap_or("-")
        )));
        if let Some(tags) = item.tags.as_ref().filter(|tags| !tags.is_empty()) {
            lines.push(Line::from(format!(
                "{} {}",
                tr("label-tags").trim_end_matches(':'),
                tags.join(", ")
            )));
        }
        let image_value = if has_detail_image {
            tr("tui-image-state-inline")
        } else if let Some(note) = app.detail_image_note.as_deref() {
            note.to_string()
        } else if image_source.is_some() {
            tr("tui-image-state-available")
        } else {
            tr("tui-image-state-no")
        };
        lines.push(Line::from(format!(
            "{}: {image_value}",
            tr("label-image").trim_end_matches(':')
        )));
        if let Some(notes) = item
            .notes
            .as_deref()
            .map(note_text_for_display)
            .filter(|v| !v.is_empty())
        {
            lines.push(Line::from(tr("label-notes")));
            lines.push(Line::from(notes));
        }
        let comments = app.comments_cache.get(&item.id);
        let comment_count =
            comments.map_or_else(|| item.comment_count.unwrap_or(0) as usize, Vec::len);
        lines.push(Line::from(format!(
            "{} {}",
            tr("label-comments").trim_end_matches(':'),
            comment_count
        )));
        if let Some(comments) = comments {
            for comment in comments.iter().rev().take(3).rev() {
                let author = comment
                    .user_name
                    .as_deref()
                    .or(comment.user_email.as_deref())
                    .unwrap_or("?");
                let text = comment.text.as_deref().unwrap_or("");
                lines.push(Line::from(format!("- {author}: {text}")));
            }
        }
    } else {
        lines.push(Line::from(item_placeholder(app)));
    }

    let widget = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(tr("label-details")),
        )
        .wrap(Wrap { trim: true });
    frame.render_widget(widget, detail_areas.1);
}

fn draw_kanban_mode(frame: &mut Frame<'_>, app: &App, area: Rect) {
    if app.items.is_empty() {
        draw_item_placeholder(frame, app, area, tr("view-board"));
        return;
    }

    let (columns, buckets) = app.kanban_buckets();
    if columns.is_empty() {
        return;
    }

    let selected_column = buckets
        .iter()
        .position(|bucket| bucket.contains(&app.selected_item));
    let selected_column_index = selected_column.unwrap_or(0);
    let (start_col, visible_count) =
        kanban_visible_range(area, columns.len(), selected_column_index);
    let chunks = kanban_chunks(area, visible_count);

    for (local_idx, chunk) in chunks.iter().enumerate().take(visible_count) {
        let col_idx = start_col + local_idx;
        let column = &columns[col_idx];
        let max_rows = chunk.height.saturating_sub(2) as usize;
        let total = buckets[col_idx].len();
        let selected_in_column = selected_column == Some(col_idx);
        let (start, item_count, show_top, show_bottom) =
            kanban_window(&buckets[col_idx], app.selected_item, max_rows);
        let end = start.saturating_add(item_count).min(total);

        let mut lines = Vec::new();
        if show_top {
            lines.push(Line::from(format!("↑{}", start)));
        }
        for item_index in buckets[col_idx].iter().skip(start).take(item_count) {
            let item = &app.items[*item_index];
            if *item_index == app.selected_item {
                lines.push(Line::styled(
                    format!(">> {}", kanban_card_text(item)),
                    Style::default()
                        .fg(Color::White)
                        .bg(SELECTED_BG)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                lines.push(Line::styled(
                    format!("   {}", kanban_card_text(item)),
                    Style::default().fg(MUTED_TEXT),
                ));
            }
        }
        if show_bottom {
            lines.push(Line::from(format!("↓{}", total - end)));
        }
        if lines.is_empty() {
            lines.push(Line::from("-"));
        }

        let border_style =
            if app.kanban_drag_item.is_some() && app.kanban_drag_target_column == Some(col_idx) {
                Style::default()
                    .fg(DRAG_TARGET_COLOR)
                    .add_modifier(Modifier::BOLD)
            } else if selected_in_column {
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

        let widget = Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(border_style)
                    .title(format!(
                        "{}{}{} ({})",
                        if local_idx == 0 && start_col > 0 {
                            "← "
                        } else {
                            ""
                        },
                        column.name,
                        if local_idx + 1 == visible_count
                            && start_col + visible_count < columns.len()
                        {
                            " →"
                        } else {
                            ""
                        },
                        buckets[col_idx].len()
                    )),
            )
            .wrap(Wrap { trim: true });
        frame.render_widget(widget, *chunk);
    }
}

fn draw_calendar_mode(frame: &mut Frame<'_>, app: &App, area: Rect) {
    if app.items.is_empty() {
        draw_item_placeholder(frame, app, area, tr("view-calendar"));
        return;
    }

    let CalendarLayout {
        title,
        month_title,
        agenda_title,
        month_area,
        agenda_area,
        month_lines,
        agenda_lines,
        ..
    } = app.calendar_layout(area);
    let border_style = if app.focus == FocusPane::Items {
        Style::default().fg(ACCENT)
    } else {
        Style::default()
    };
    let calendar_dragging = app.calendar_drag_started && app.calendar_drag_item.is_some();
    let month_border_style = if calendar_dragging && app.calendar_drag_target_date.is_some() {
        Style::default()
            .fg(DRAG_TARGET_COLOR)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let agenda_border_style = if calendar_dragging && app.calendar_drag_target_date.is_none() {
        Style::default()
            .fg(DRAG_TARGET_COLOR)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(border_style),
        area,
    );

    if month_area.width >= 2 && month_area.height >= 2 {
        frame.render_widget(
            Paragraph::new(month_lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(month_border_style)
                    .title(month_title),
            ),
            month_area,
        );
    }

    if agenda_area.width >= 2 && agenda_area.height >= 2 {
        frame.render_widget(
            Paragraph::new(agenda_lines)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(agenda_border_style)
                        .title(agenda_title),
                )
                .wrap(Wrap { trim: true }),
            agenda_area,
        );
    }
}

fn item_placeholder(app: &App) -> String {
    if app.loading_items_for == app.selected_list_id() {
        tr_args(
            "tui-items-loading",
            &[("list", app.selected_list_display_name())],
        )
    } else {
        tr("output-no-items")
    }
}

fn draw_item_placeholder(frame: &mut Frame<'_>, app: &App, area: Rect, title: String) {
    let widget = Paragraph::new(item_placeholder(app))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL).title(title));
    frame.render_widget(widget, area);
}

fn draw_footer(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let block = Block::default().borders(Borders::ALL);
    frame.render_widget(block, area);

    for (action, rect) in footer_buttons(area, &app.key_bindings) {
        let widget = Paragraph::new(action_chip_text(action, &app.key_bindings))
            .style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD));
        frame.render_widget(widget, rect);
    }

    if let Some(status) = app.status.as_ref() {
        let status_widget = Paragraph::new(status.clone()).style(
            Style::default()
                .fg(STATUS_COLOR)
                .add_modifier(Modifier::BOLD),
        );
        let inner = area.inner(Margin {
            vertical: 1,
            horizontal: 1,
        });
        let status_area = Rect::new(
            inner.x,
            inner.y.saturating_add(inner.height.saturating_sub(1)),
            inner.width,
            1,
        );
        frame.render_widget(status_widget, status_area);
    }
}

fn action_chip_text(action: FooterAction, key_bindings: &KeyBindings) -> String {
    format!(
        "[{}] {}",
        key_bindings.label_for(action),
        action.chip_label()
    )
}

fn draw_beta_consent_overlay(frame: &mut Frame<'_>, app: &mut App) {
    let layout = beta_consent_layout(frame.area());
    frame.render_widget(Clear, layout.outer);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .title(tr("tui-beta-consent-title"))
            .border_style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        layout.outer,
    );

    let profile_text = app
        .profile_name
        .clone()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| tr("common-unknown"));
    let body = vec![
        Line::styled(profile_text, Style::default().add_modifier(Modifier::BOLD)),
        Line::from(""),
        Line::from(tr("cli-interactive-beta-notice")),
        Line::from(""),
        Line::from(tr("tui-beta-consent-body")),
    ];
    let mut text_area = layout.body;
    if app.profile_image.is_some() && layout.body.width >= 44 && layout.body.height >= 4 {
        let image_width = layout.body.height.saturating_mul(2).clamp(8, 16);
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(image_width),
                Constraint::Length(2),
                Constraint::Min(20),
            ])
            .split(layout.body);
        if let Some(profile_image) = app.profile_image.as_mut() {
            frame.render_stateful_widget(
                StatefulImage::default().resize(Resize::Fit(Some(FilterType::Lanczos3))),
                chunks[0],
                &mut profile_image.protocol,
            );
        }
        text_area = chunks[2];
    }
    frame.render_widget(
        Paragraph::new(body)
            .alignment(if text_area == layout.body {
                Alignment::Center
            } else {
                Alignment::Left
            })
            .wrap(Wrap { trim: true })
            .style(Style::default().fg(Color::Reset)),
        text_area,
    );

    frame.render_widget(
        Paragraph::new(tr("tui-beta-consent-hint"))
            .alignment(Alignment::Center)
            .style(
                Style::default()
                    .fg(Color::Reset)
                    .add_modifier(Modifier::DIM),
            ),
        layout.hint,
    );

    let accept_label = format!("[Enter] {}", tr("tui-beta-consent-accept"));
    let decline_label = format!("[Esc] {}", tr("tui-beta-consent-decline"));
    frame.render_widget(
        Paragraph::new(accept_label)
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::White).bg(SAVE_BG)),
        layout.accept,
    );
    frame.render_widget(
        Paragraph::new(decline_label)
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::White).bg(CANCEL_BG)),
        layout.decline,
    );
}

fn draw_legal_consent_overlay(frame: &mut Frame<'_>, app: &App) {
    let layout = beta_consent_layout(frame.area());
    frame.render_widget(Clear, layout.outer);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .title(tr("tui-legal-consent-title"))
            .border_style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        layout.outer,
    );

    let docs = if app.legal_pending_docs.is_empty() {
        "-".to_string()
    } else {
        app.legal_pending_docs.join(", ")
    };
    let body = vec![
        Line::from(tr("tui-legal-consent-body")),
        Line::from(""),
        Line::from(tr_args("tui-legal-consent-pending", &[("docs", docs)])),
    ];
    frame.render_widget(
        Paragraph::new(body)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true })
            .style(Style::default().fg(Color::Reset)),
        layout.body,
    );

    let hint = if app.legal_accepting {
        tr("tui-legal-consent-submitting")
    } else {
        tr("tui-legal-consent-hint")
    };
    frame.render_widget(
        Paragraph::new(hint).alignment(Alignment::Center).style(
            Style::default()
                .fg(Color::Reset)
                .add_modifier(Modifier::DIM),
        ),
        layout.hint,
    );

    let accept_label = format!("[Enter] {}", tr("tui-legal-consent-accept"));
    let decline_label = format!("[Esc] {}", tr("tui-legal-consent-decline"));
    let accept_style = if app.legal_accepting {
        Style::default().fg(Color::White).bg(MUTED_TEXT)
    } else {
        Style::default().fg(Color::White).bg(SAVE_BG)
    };
    let decline_style = if app.legal_accepting {
        Style::default().fg(MUTED_TEXT)
    } else {
        Style::default().fg(Color::White).bg(CANCEL_BG)
    };
    frame.render_widget(
        Paragraph::new(accept_label)
            .alignment(Alignment::Center)
            .style(accept_style),
        layout.accept,
    );
    frame.render_widget(
        Paragraph::new(decline_label)
            .alignment(Alignment::Center)
            .style(decline_style),
        layout.decline,
    );
}

fn draw_editor(frame: &mut Frame<'_>, editor: &EditorState) {
    let area = frame.area();
    let layout = editor_layout(area);

    let fields = editor_fields(editor.mode);
    let step = editor_step_index(editor);

    frame.render_widget(Clear, layout.outer);
    let title = match editor.mode {
        EditorMode::Create => tr("label-items"),
        EditorMode::Edit => tr("label-changes"),
        EditorMode::Comment => tr("label-comments"),
        EditorMode::Filter => tr("label-text"),
    };
    frame.render_widget(
        Block::default().borders(Borders::ALL).title(format!(
            "{} ({}/{})",
            title,
            step.saturating_add(1),
            fields.len()
        )),
        layout.outer,
    );

    let mut progress = String::default();
    for (index, _) in fields.iter().enumerate() {
        if index > 0 {
            progress.push(' ');
        }
        if index == step {
            progress.push('●');
        } else {
            progress.push('○');
        }
    }
    let progress_text = format!(
        "{} {}/{}  {}",
        tr("tui-editor-step"),
        step.saturating_add(1),
        fields.len(),
        progress
    );
    frame.render_widget(
        Paragraph::new(progress_text)
            .style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))
            .alignment(Alignment::Center),
        layout.progress,
    );
    render_editor_field(frame, layout.field, editor.active_field, editor);
    frame.render_widget(
        Paragraph::new(format!(
            "{}  ·  ← →  ·  [^S]  ·  [⎋]",
            editor_field_hint(editor.active_field, editor.mode).trim_end_matches(':')
        ))
        .style(Style::default().fg(MUTED_TEXT)),
        layout.hint,
    );

    let save_style = Style::default().fg(Color::White).bg(SAVE_BG);
    let cancel_style = Style::default().fg(Color::White).bg(CANCEL_BG);
    let nav_style = Style::default().fg(Color::White).bg(SELECTED_BG);
    frame.render_widget(Paragraph::new(" [←] ").style(nav_style), layout.prev);
    frame.render_widget(Paragraph::new(" [→] ").style(nav_style), layout.next);
    frame.render_widget(Paragraph::new(" [✓] ").style(save_style), layout.save);
    frame.render_widget(Paragraph::new(" [✕] ").style(cancel_style), layout.cancel);
}

fn draw_help_overlay(frame: &mut Frame<'_>) {
    let area = frame.area();
    let (popup, inner) = centered_popup(area, 54, 92, 18, 28);

    frame.render_widget(Clear, popup);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .title(tr("tui-help-title"))
            .border_style(Style::default().fg(ACCENT)),
        popup,
    );

    let text = vec![
        Line::from(vec![Span::styled(
            tr("tui-help-navigation"),
            Style::default().add_modifier(Modifier::BOLD),
        )]),
        Line::from(tr("tui-help-nav-1")),
        Line::from(tr("tui-help-nav-2")),
        Line::from(tr("tui-help-nav-3")),
        Line::from(tr("tui-help-nav-4")),
        Line::from(tr("tui-help-nav-5")),
        Line::from(""),
        Line::from(vec![Span::styled(
            tr("tui-help-calendar"),
            Style::default().add_modifier(Modifier::BOLD),
        )]),
        Line::from(tr("tui-help-calendar-1")),
        Line::from(tr("tui-help-calendar-2")),
        Line::from(tr("tui-help-calendar-3")),
        Line::from(""),
        Line::from(vec![Span::styled(
            tr("tui-help-actions"),
            Style::default().add_modifier(Modifier::BOLD),
        )]),
        Line::from(tr("tui-help-actions-1")),
        Line::from(tr("tui-help-actions-2")),
        Line::from(tr("tui-help-actions-3")),
        Line::from(tr("tui-help-actions-4")),
        Line::from(""),
        Line::from(vec![Span::styled(
            tr("tui-help-editor"),
            Style::default().add_modifier(Modifier::BOLD),
        )]),
        Line::from(tr("tui-help-editor-1")),
        Line::from(tr("tui-help-editor-2")),
        Line::from(tr("tui-help-editor-3")),
        Line::from(tr("tui-help-editor-4")),
        Line::from(""),
        Line::from(tr("tui-help-close")),
    ];

    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: true })
            .style(Style::default().fg(MUTED_TEXT)),
        inner,
    );
}

fn render_editor_field(
    frame: &mut Frame<'_>,
    rect: Rect,
    field: EditorField,
    editor: &EditorState,
) {
    let value = match field {
        EditorField::Text => &editor.text,
        EditorField::Quantity => &editor.quantity,
        EditorField::DueDate => &editor.due_date,
        EditorField::DueTime => &editor.due_time,
        EditorField::PlannedDate => &editor.planned_date,
        EditorField::PlannedTime => &editor.planned_time,
        EditorField::Reminder => &editor.reminder,
        EditorField::ReminderTime => &editor.reminder_time,
        EditorField::ReminderOffsets => &editor.reminder_offsets,
        EditorField::TravelTimeMinutes => &editor.travel_time_minutes,
        EditorField::Priority => &editor.priority,
        EditorField::Tags => &editor.tags,
        EditorField::Progress => &editor.progress,
        EditorField::Notes => &editor.notes,
    };

    let label = field.label();
    let active = field == editor.active_field;
    let border_style = if active {
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(MUTED_TEXT)
    };
    let text_style = if active {
        Style::default().fg(Color::White).bg(SELECTED_BG)
    } else {
        Style::default()
    };
    let text = if value.trim().is_empty() {
        "—".to_string()
    } else {
        value.to_string()
    };
    let widget = Paragraph::new(text)
        .style(text_style)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(label.trim_end_matches(':')),
        )
        .wrap(Wrap { trim: true });
    frame.render_widget(widget, rect);
}

fn rect_contains(rect: Rect, x: u16, y: u16) -> bool {
    x >= rect.x
        && x < rect.x.saturating_add(rect.width)
        && y >= rect.y
        && y < rect.y.saturating_add(rect.height)
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum ImageProtocolPreference {
    Off,
    Auto,
    Forced(ProtocolType),
}

impl ImageProtocolPreference {
    fn shows_inline_images(self) -> bool {
        !matches!(self, Self::Off)
    }
}

fn image_protocol_preference() -> ImageProtocolPreference {
    if let Ok(raw) = std::env::var(KRAMLI_TUI_IMAGE_PROTOCOL_ENV) {
        let value = raw.trim().to_ascii_lowercase();
        return match value.as_str() {
            "off" | "none" | "disabled" | "0" => ImageProtocolPreference::Off,
            "auto" | "" => ImageProtocolPreference::Auto,
            "kitty" => ImageProtocolPreference::Forced(ProtocolType::Kitty),
            "sixel" => ImageProtocolPreference::Forced(ProtocolType::Sixel),
            "iterm2" | "iterm" | "imgcat" => ImageProtocolPreference::Forced(ProtocolType::Iterm2),
            "halfblocks" | "text" | "ascii" => {
                ImageProtocolPreference::Forced(ProtocolType::Halfblocks)
            }
            _ => ImageProtocolPreference::Auto,
        };
    }

    if std::env::var(KRAMLI_TUI_IMAGES_ENV).is_ok_and(|value| value == "0") {
        return ImageProtocolPreference::Off;
    }

    ImageProtocolPreference::Auto
}

fn protocol_type_name(protocol: ProtocolType) -> &'static str {
    match protocol {
        ProtocolType::Halfblocks => "halfblocks",
        ProtocolType::Sixel => "sixel",
        ProtocolType::Kitty => "kitty",
        ProtocolType::Iterm2 => "imgcat",
    }
}

fn image_preference_name(preference: ImageProtocolPreference) -> &'static str {
    match preference {
        ImageProtocolPreference::Off => "off",
        ImageProtocolPreference::Auto => "auto",
        ImageProtocolPreference::Forced(ProtocolType::Halfblocks) => "halfblocks",
        ImageProtocolPreference::Forced(ProtocolType::Sixel) => "sixel",
        ImageProtocolPreference::Forced(ProtocolType::Kitty) => "kitty",
        ImageProtocolPreference::Forced(ProtocolType::Iterm2) => "imgcat",
    }
}

fn image_runtime_debug_lines(
    preference: ImageProtocolPreference,
    mode: &str,
    picker: &Picker,
    inline_enabled: bool,
) -> Vec<String> {
    let term = std::env::var(TERM_ENV).unwrap_or_else(|_| "-".to_string());
    let term_program = std::env::var(TERM_PROGRAM_ENV).unwrap_or_else(|_| "-".to_string());
    let lc_terminal = std::env::var(LC_TERMINAL_ENV).unwrap_or_else(|_| "-".to_string());
    let explicit_protocol = std::env::var(KRAMLI_TUI_IMAGE_PROTOCOL_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "(unset)".to_string());
    let explicit_images = std::env::var(KRAMLI_TUI_IMAGES_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "(unset)".to_string());

    vec![
        format!(
            "img pref={} mode={} inline={}",
            image_preference_name(preference),
            mode,
            if inline_enabled { "on" } else { "off" }
        ),
        format!(
            "img protocol={} caps={}",
            protocol_type_name(picker.protocol_type()),
            picker.capabilities().len()
        ),
        format!("img term={} term_program={}", term, term_program),
        format!(
            "img lc_terminal={} iterm_session={}",
            lc_terminal,
            if std::env::var(ITERM_SESSION_ID_ENV).is_ok() {
                "set"
            } else {
                "unset"
            }
        ),
        format!(
            "img env protocol={} images={}",
            explicit_protocol, explicit_images
        ),
    ]
}

fn autodetect_protocol_fallback() -> Option<ProtocolType> {
    let term = std::env::var(TERM_ENV)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let term_program = std::env::var(TERM_PROGRAM_ENV)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let lc_terminal = std::env::var(LC_TERMINAL_ENV)
        .unwrap_or_default()
        .to_ascii_lowercase();
    detected_protocol_from_env_values(
        &term,
        &term_program,
        &lc_terminal,
        std::env::var(KITTY_WINDOW_ID_ENV).is_ok(),
        std::env::var(ITERM_SESSION_ID_ENV).is_ok(),
        std::env::var(WT_SESSION_ENV).is_ok(),
    )
}

fn detected_protocol_from_env_values(
    term: &str,
    term_program: &str,
    lc_terminal: &str,
    has_kitty_window: bool,
    has_iterm_session: bool,
    has_windows_terminal: bool,
) -> Option<ProtocolType> {
    if terminal_mentions_any(&[term, term_program], &["alacritty"]) {
        return None;
    }
    if detected_kitty_protocol(term, term_program, has_kitty_window) {
        return Some(ProtocolType::Kitty);
    }
    if detected_sixel_protocol(term, term_program, has_windows_terminal) {
        return Some(ProtocolType::Sixel);
    }
    if detected_iterm_protocol(term, term_program, lc_terminal, has_iterm_session) {
        return Some(ProtocolType::Iterm2);
    }
    None
}

fn terminal_mentions_any(values: &[&str], needles: &[&str]) -> bool {
    values
        .iter()
        .any(|value| needles.iter().any(|needle| value.contains(needle)))
}

fn detected_kitty_protocol(term: &str, term_program: &str, has_kitty_window: bool) -> bool {
    has_kitty_window
        || terminal_mentions_any(&[term, term_program], &["kitty", "ghostty", "konsole"])
}

fn detected_sixel_protocol(term: &str, term_program: &str, has_windows_terminal: bool) -> bool {
    has_windows_terminal
        || term.contains("sixel")
        || terminal_mentions_any(&[term, term_program], &["foot", "blackbox"])
        || term.contains("zellij")
}

fn detected_iterm_protocol(
    term: &str,
    term_program: &str,
    lc_terminal: &str,
    has_iterm_session: bool,
) -> bool {
    has_iterm_session
        || lc_terminal.contains("iterm")
        || terminal_mentions_any(
            &[term, term_program],
            &[
                "iterm", "wezterm", "warp", "tabby", "vscode", "bobcat", "rio",
            ],
        )
}

fn build_image_picker(preference: ImageProtocolPreference) -> (Picker, bool, String, Vec<String>) {
    let label = tr("label-image").trim_end_matches(':').to_string();
    match preference {
        ImageProtocolPreference::Off => {
            let picker = Picker::halfblocks();
            let summary = format!("{label} auto=off");
            let debug = image_runtime_debug_lines(preference, "off", &picker, false);
            (picker, false, summary, debug)
        }
        ImageProtocolPreference::Auto => {
            if should_probe_terminal_images() {
                let mut picker =
                    Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
                let mut suffix = "probe";
                if let Some(protocol) = env_override_for_probed_protocol(
                    picker.protocol_type(),
                    autodetect_protocol_fallback(),
                ) {
                    picker.set_protocol_type(protocol);
                    suffix = match protocol {
                        ProtocolType::Iterm2 => "probe+iterm",
                        _ => "probe+env",
                    };
                }
                let summary = format!(
                    "{label} auto={} ({suffix})",
                    protocol_type_name(picker.protocol_type())
                );
                let debug = image_runtime_debug_lines(preference, suffix, &picker, true);
                (picker, true, summary, debug)
            } else {
                // Probe can be skipped for known terminals, but inline images
                // should still work out of the box via halfblocks.
                let picker = Picker::halfblocks();
                let summary = format!(
                    "{label} auto={} (safe)",
                    protocol_type_name(picker.protocol_type())
                );
                let debug = image_runtime_debug_lines(preference, "safe", &picker, true);
                (picker, true, summary, debug)
            }
        }
        ImageProtocolPreference::Forced(ProtocolType::Halfblocks) => {
            let picker = Picker::halfblocks();
            let summary = format!("{label} set={}", protocol_type_name(picker.protocol_type()));
            let debug = image_runtime_debug_lines(preference, "forced", &picker, true);
            (picker, true, summary, debug)
        }
        ImageProtocolPreference::Forced(protocol) => {
            let mut picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
            picker.set_protocol_type(protocol);
            let summary = format!("{label} set={}", protocol_type_name(protocol));
            let debug = image_runtime_debug_lines(preference, "forced", &picker, true);
            (picker, true, summary, debug)
        }
    }
}

fn env_override_for_probed_protocol(
    probed: ProtocolType,
    env_protocol: Option<ProtocolType>,
) -> Option<ProtocolType> {
    match (probed, env_protocol) {
        (ProtocolType::Halfblocks, Some(protocol)) => Some(protocol),
        (ProtocolType::Kitty, Some(ProtocolType::Iterm2)) => Some(ProtocolType::Iterm2),
        _ => None,
    }
}

fn should_probe_terminal_images() -> bool {
    if std::env::var(KRAMLI_TUI_IMAGES_ENV).is_ok_and(|value| value == "0") {
        return false;
    }
    if std::env::var(KRAMLI_TUI_IMAGES_ENV).is_ok_and(|value| value == "1") {
        return true;
    }

    let term = std::env::var(TERM_ENV)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let term_program = std::env::var(TERM_PROGRAM_ENV)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let lc_terminal = std::env::var(LC_TERMINAL_ENV)
        .unwrap_or_default()
        .to_ascii_lowercase();

    detected_protocol_from_env_values(
        &term,
        &term_program,
        &lc_terminal,
        std::env::var(KITTY_WINDOW_ID_ENV).is_ok(),
        std::env::var(ITERM_SESSION_ID_ENV).is_ok(),
        std::env::var(WT_SESSION_ENV).is_ok(),
    )
    .is_some()
}

fn profile_pending_legal_docs(profile: &Profile) -> Vec<String> {
    let mut docs = Vec::new();
    let pending = profile
        .legal
        .as_ref()
        .map_or(&[][..], |legal| legal.pending.as_slice());
    for doc in pending {
        if let Some(key) = doc
            .key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            push_unique_case_insensitive(&mut docs, key);
        }
    }
    docs
}

fn pending_legal_docs_from_value(value: &Value) -> Vec<String> {
    let mut docs = Vec::new();
    let pending = value
        .get("legal")
        .and_then(|legal| legal.get("pending"))
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    for doc in pending {
        if let Some(key) = doc
            .get("key")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            push_unique_case_insensitive(&mut docs, key);
        }
    }
    docs
}

fn list_states_to_columns(states: Option<&[ApiListState]>) -> Vec<KanbanColumn> {
    let mut columns = Vec::new();
    if let Some(states) = states {
        for ApiListState { name, is_done, .. } in states {
            if let Some(name) = name
                .as_ref()
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
            {
                columns.push(KanbanColumn {
                    name: name.to_string(),
                    is_done: is_done.unwrap_or(false),
                });
            }
        }
    }
    normalize_kanban_columns(columns)
}

fn parse_state_config_columns(state_config: Option<&str>) -> Vec<KanbanColumn> {
    let Some(raw) = state_config
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Vec::new();
    };

    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    let Some(entries) = value.as_array() else {
        return Vec::new();
    };

    let mut columns = Vec::new();
    for entry in entries {
        match entry {
            Value::String(name) => {
                let name = name.trim();
                if !name.is_empty() {
                    columns.push(KanbanColumn {
                        name: name.to_string(),
                        is_done: false,
                    });
                }
            }
            Value::Object(map) => {
                let Some(name) = map
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                else {
                    continue;
                };
                let is_done = map
                    .get("is_done")
                    .or_else(|| map.get("isDone"))
                    .or_else(|| map.get("done"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                columns.push(KanbanColumn {
                    name: name.to_string(),
                    is_done,
                });
            }
            _ => {}
        }
    }

    normalize_kanban_columns(columns)
}

fn normalize_kanban_columns(columns: Vec<KanbanColumn>) -> Vec<KanbanColumn> {
    let mut deduped = Vec::new();
    for column in columns {
        if deduped
            .iter()
            .any(|existing: &KanbanColumn| existing.name.eq_ignore_ascii_case(&column.name))
        {
            continue;
        }
        deduped.push(column);
    }

    if !deduped.is_empty() && deduped.iter().all(|column| !column.is_done) {
        if let Some(last) = deduped.last_mut() {
            last.is_done = true;
        }
    }

    deduped
}

fn shifted_index(current: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    current
        .saturating_add_signed(delta)
        .min(len.saturating_sub(1))
}

fn wrapped_index(current: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let len_isize = len as isize;
    let current_isize = current.min(len.saturating_sub(1)) as isize;
    let wrapped = (current_isize + delta).rem_euclid(len_isize);
    wrapped as usize
}

fn push_unique_case_insensitive(target: &mut Vec<String>, value: &str) {
    if target.iter().any(|item| item.eq_ignore_ascii_case(value)) {
        return;
    }
    target.push(value.to_string());
}

fn cycle_suggestion_value(current: &str, suggestions: &[String], delta: isize) -> Option<String> {
    if delta == 0 || suggestions.is_empty() {
        return None;
    }

    let current = current.trim();
    if let Some(current_index) = suggestions
        .iter()
        .position(|value| value.eq_ignore_ascii_case(current))
    {
        let next_index = wrapped_index(current_index, delta.signum(), suggestions.len());
        return suggestions.get(next_index).cloned();
    }

    let prefix = current.to_ascii_lowercase();
    let filtered = suggestions
        .iter()
        .filter(|value| {
            prefix.is_empty() || value.to_ascii_lowercase().starts_with(prefix.as_str())
        })
        .collect::<Vec<_>>();

    if filtered.is_empty() {
        return None;
    }

    if delta.is_negative() {
        filtered.last().cloned().cloned()
    } else {
        filtered.first().cloned().cloned()
    }
}

fn autocomplete_last_tag(raw: &str, suggestions: &[String], delta: isize) -> Option<String> {
    if delta == 0 || suggestions.is_empty() {
        return None;
    }

    let (prefix, tail) = if let Some((head, last)) = raw.rsplit_once(',') {
        (format!("{head},"), last)
    } else {
        (String::default(), raw)
    };

    let leading_ws = tail.chars().take_while(|ch| ch.is_whitespace()).count();
    let current_tag = tail.trim();
    let replacement = cycle_suggestion_value(current_tag, suggestions, delta)?;

    let spacer = if prefix.is_empty() {
        " ".repeat(leading_ws)
    } else if leading_ws == 0 {
        " ".to_string()
    } else {
        " ".repeat(leading_ws)
    };

    Some(format!("{prefix}{spacer}{replacement}"))
}

fn scroll_to_visible(scroll: usize, selected: usize, visible_rows: usize) -> usize {
    if visible_rows == 0 || selected < scroll {
        return selected;
    }
    let bottom = scroll.saturating_add(visible_rows);
    if selected >= bottom {
        selected.saturating_add(1).saturating_sub(visible_rows)
    } else {
        scroll
    }
}

fn next_kanban_selection(
    buckets: &[Vec<usize>],
    selected_item: usize,
    delta: isize,
) -> Option<usize> {
    let selected_column = buckets
        .iter()
        .position(|bucket| bucket.contains(&selected_item))
        .or_else(|| buckets.iter().position(|bucket| !bucket.is_empty()))?;
    let bucket = &buckets[selected_column];
    let current_row = bucket
        .iter()
        .position(|item_index| *item_index == selected_item)
        .unwrap_or(0);
    let next_row = shifted_index(current_row, delta, bucket.len());
    bucket.get(next_row).copied()
}

fn next_kanban_column_selection(
    buckets: &[Vec<usize>],
    selected_item: usize,
    delta: isize,
) -> Option<usize> {
    if delta == 0 || buckets.is_empty() {
        return Some(selected_item);
    }

    let selected_column = buckets
        .iter()
        .position(|bucket| bucket.contains(&selected_item))
        .or_else(|| buckets.iter().position(|bucket| !bucket.is_empty()))?;

    let current_row = buckets[selected_column]
        .iter()
        .position(|item_index| *item_index == selected_item)
        .unwrap_or(0);

    let step = delta.signum();
    let mut col = selected_column;
    loop {
        let next_col = shifted_index(col, step, buckets.len());
        if next_col == col {
            return None;
        }
        col = next_col;
        let bucket = &buckets[col];
        if bucket.is_empty() {
            continue;
        }
        let target_row = current_row.min(bucket.len().saturating_sub(1));
        return bucket.get(target_row).copied();
    }
}

fn stepped_kanban_selection(
    buckets: &[Vec<usize>],
    selected_item: usize,
    delta: isize,
) -> Option<usize> {
    if delta == 0 {
        return Some(selected_item);
    }

    let direction = if delta > 0 { 1 } else { -1 };
    let mut current = selected_item;
    let mut moved = false;
    for _ in 0..delta.unsigned_abs() {
        let Some(next) = next_kanban_selection(buckets, current, direction) else {
            break;
        };
        if next == current {
            break;
        }
        current = next;
        moved = true;
    }

    if moved {
        Some(current)
    } else {
        None
    }
}

fn stepped_kanban_selection_wrapped(
    buckets: &[Vec<usize>],
    selected_item: usize,
    delta: isize,
) -> Option<usize> {
    if delta == 0 {
        return Some(selected_item);
    }

    let selected_column = buckets
        .iter()
        .position(|bucket| bucket.contains(&selected_item))
        .or_else(|| buckets.iter().position(|bucket| !bucket.is_empty()))?;
    let bucket = &buckets[selected_column];
    if bucket.is_empty() {
        return None;
    }

    let mut row = bucket
        .iter()
        .position(|item_index| *item_index == selected_item)
        .unwrap_or(0);
    let direction = delta.signum();
    for _ in 0..delta.unsigned_abs() {
        row = wrapped_index(row, direction, bucket.len());
    }

    bucket.get(row).copied()
}

fn kanban_window(
    bucket: &[usize],
    selected_item: usize,
    max_rows: usize,
) -> (usize, usize, bool, bool) {
    if bucket.is_empty() || max_rows == 0 {
        return (0, 0, false, false);
    }

    let len = bucket.len();
    if len <= max_rows {
        return (0, len, false, false);
    }

    let selected_pos = bucket
        .iter()
        .position(|item| *item == selected_item)
        .unwrap_or(0);

    for count in (1..=max_rows.min(len)).rev() {
        let mut start = selected_pos.saturating_sub(count / 2);
        if start + count > len {
            start = len - count;
        }
        if selected_pos < start {
            start = selected_pos;
        }
        if selected_pos >= start + count {
            start = selected_pos + 1 - count;
        }

        let show_top = start > 0;
        let show_bottom = start + count < len;
        let rows_used = count + show_top as usize + show_bottom as usize;
        if rows_used <= max_rows {
            return (start, count, show_top, show_bottom);
        }
    }

    (0, 1, false, true)
}

fn kanban_column_at(chunks: &[Rect], x: u16, y: u16) -> Option<usize> {
    chunks.iter().position(|rect| rect_contains(*rect, x, y))
}

fn item_row_text(item: &ListItem) -> String {
    let mut out = String::default();
    let depth = item.depth.unwrap_or(0).max(0) as usize;
    if depth > 0 {
        out.push_str(&"  ".repeat(depth));
    }
    out.push_str(if item.is_done.unwrap_or(false) {
        "[x] "
    } else {
        "[ ] "
    });
    out.push_str(&item.text);

    if let Some(quantity) = item
        .quantity
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        out.push_str(" x");
        out.push_str(quantity);
    }

    if has_image(item) {
        out.push_str(" [img]");
    }

    if let Some(progress) = item
        .progress
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        out.push_str(" | ");
        out.push_str(progress);
    }

    if let Some(due) = item
        .due_date
        .as_deref()
        .and_then(|value| value.get(0..10))
        .filter(|value| !value.is_empty())
    {
        out.push_str(" | ");
        out.push_str(due);
        if let Some(time) = item
            .due_time
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            out.push(' ');
            out.push_str(time);
        }
    }

    out
}

fn apply_item_depths(items: &mut [ListItem]) {
    let parent_by_id: HashMap<i64, Option<i64>> = items
        .iter()
        .map(|item| (item.id, item.parent_item_id))
        .collect();

    for item in items {
        if item.depth.unwrap_or(0) > 0 {
            continue;
        }
        let mut depth = 0;
        let mut parent = item.parent_item_id;
        let mut seen = HashSet::new();
        while let Some(parent_id) = parent {
            if !seen.insert(parent_id) {
                break;
            }
            let Some(next_parent) = parent_by_id.get(&parent_id) else {
                break;
            };
            depth += 1;
            parent = *next_parent;
        }
        if depth > 0 {
            item.depth = Some(depth);
        }
    }
}

fn kanban_card_text(item: &ListItem) -> String {
    let mut out = String::default();
    out.push_str(if item.is_done.unwrap_or(false) {
        "[x] "
    } else {
        "[ ] "
    });
    out.push_str(&item.text);

    if let Some(quantity) = item
        .quantity
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        out.push_str(" x");
        out.push_str(quantity);
    }

    if let Some(due) = item
        .due_date
        .as_deref()
        .and_then(|value| value.get(0..10))
        .filter(|value| !value.is_empty())
    {
        out.push_str(" · ");
        out.push_str(due);
        if let Some(time) = item
            .due_time
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            out.push(' ');
            out.push_str(time);
        }
    }

    if let Some(count) = item.comment_count.filter(|count| *count > 0) {
        out.push_str(&format!(" · c{count}"));
    }

    if has_image(item) {
        out.push_str(" · [img]");
    }

    out
}

fn date_with_time_display(date: Option<&str>, time: Option<&str>) -> String {
    let Some(date) = date
        .and_then(|value| value.get(0..10))
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return "-".to_string();
    };
    match time.map(str::trim).filter(|value| !value.is_empty()) {
        Some(time) => format!("{date} {time}"),
        None => date.to_string(),
    }
}

fn reminder_offsets_display(offsets: &[i64]) -> String {
    offsets
        .iter()
        .map(|offset| {
            if *offset >= 1440 && offset % 1440 == 0 {
                format!("{}d", offset / 1440)
            } else if *offset >= 60 && offset % 60 == 0 {
                format!("{}h", offset / 60)
            } else {
                format!("{offset}m")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn item_matches_filter(item: &ListItem, query: &str) -> bool {
    item.text.to_ascii_lowercase().contains(query)
        || item
            .quantity
            .as_deref()
            .is_some_and(|value| value.to_ascii_lowercase().contains(query))
        || item.notes.as_deref().is_some_and(|value| {
            note_text_for_display(value)
                .to_ascii_lowercase()
                .contains(query)
        })
        || item
            .priority
            .as_deref()
            .is_some_and(|value| value.to_ascii_lowercase().contains(query))
        || item.tags.as_ref().is_some_and(|tags| {
            tags.iter()
                .any(|tag| tag.to_ascii_lowercase().contains(query))
        })
}

fn note_text_for_editor(raw: &str) -> String {
    note_text_for_display(raw)
}

fn note_text_for_display(raw: &str) -> String {
    let decoded_source = decode_html_entities(raw);
    let text_source = remove_html_non_text_blocks(&decoded_source);
    let with_breaks = html_breaks_to_newlines(&text_source);
    let without_tags = strip_html_tags(&with_breaks);
    decode_html_entities(&without_tags)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn remove_html_non_text_blocks(raw: &str) -> String {
    let mut text = raw.to_string();
    for tag in [
        "script", "style", "svg", "canvas", "math", "head", "noscript",
    ] {
        text = remove_html_block(&text, tag);
    }
    text
}

fn remove_html_block(raw: &str, tag_name: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut index = 0usize;
    let lower = raw.to_ascii_lowercase();
    let open = format!("<{tag_name}");
    let close = format!("</{tag_name}>");

    while let Some(relative_start) = lower[index..].find(&open) {
        let start = index + relative_start;
        out.push_str(&raw[index..start]);
        let after_open = lower[start..]
            .find('>')
            .map_or(raw.len(), |offset| start + offset + 1);
        if let Some(relative_close) = lower[after_open..].find(&close) {
            index = after_open + relative_close + close.len();
        } else {
            index = raw.len();
        }
    }

    out.push_str(&raw[index..]);
    out
}

fn html_breaks_to_newlines(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut index = 0;
    let lower = raw.to_ascii_lowercase();
    while let Some(relative) = lower[index..].find('<') {
        let start = index + relative;
        out.push_str(&raw[index..start]);
        let Some(end) = lower[start..].find('>') else {
            out.push_str(&raw[start..]);
            return out;
        };
        let raw_tag = lower[start + 1..start + end].trim();
        let closing = raw_tag.starts_with('/');
        let tag = raw_tag.trim_start_matches('/').trim();
        let tag_name = tag
            .split(|ch: char| ch.is_whitespace() || ch == '/' || ch == '>')
            .next()
            .unwrap_or(tag);
        if tag_name == "br" || tag_name == "hr" {
            out.push('\n');
        } else if tag_name == "li" && !closing {
            out.push_str("\n- ");
        } else if matches!(
            tag_name,
            "p" | "div"
                | "section"
                | "article"
                | "header"
                | "footer"
                | "blockquote"
                | "tr"
                | "table"
                | "ul"
                | "ol"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
        ) {
            out.push('\n');
        } else if matches!(tag_name, "td" | "th") {
            out.push(' ');
        }
        out.push_str(&raw[start..=start + end]);
        index = start + end + 1;
    }
    out.push_str(&raw[index..]);
    out
}

fn strip_html_tags(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut in_tag = false;
    for ch in raw.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

fn decode_html_entities(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(pos) = rest.find('&') {
        out.push_str(&rest[..pos]);
        let entity_start = pos + 1;
        let Some(end) = rest[entity_start..].find(';') else {
            out.push_str(&rest[pos..]);
            return out;
        };
        let entity = &rest[entity_start..entity_start + end];
        if let Some(ch) = decode_html_entity(entity) {
            out.push(ch);
        } else {
            out.push('&');
            out.push_str(entity);
            out.push(';');
        }
        rest = &rest[entity_start + end + 1..];
    }
    out.push_str(rest);
    out
}

fn decode_html_entity(entity: &str) -> Option<char> {
    match entity {
        "nbsp" => Some(' '),
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ if entity.starts_with("#x") || entity.starts_with("#X") => {
            u32::from_str_radix(&entity[2..], 16)
                .ok()
                .and_then(char::from_u32)
        }
        _ if entity.starts_with('#') => entity[1..].parse::<u32>().ok().and_then(char::from_u32),
        _ => None,
    }
}

fn item_short_text(item: &ListItem) -> String {
    let mut out = item.text.clone();
    if has_image(item) {
        out.push_str(" [img]");
    }
    out
}

fn sort_lists_for_tui(lists: &mut [ShoppingList]) {
    lists.sort_by(compare_lists_for_tui);
}

fn compare_lists_for_tui(a: &ShoppingList, b: &ShoppingList) -> Ordering {
    list_folder_sort_key(a)
        .cmp(&list_folder_sort_key(b))
        .then_with(|| {
            a.name
                .to_ascii_lowercase()
                .cmp(&b.name.to_ascii_lowercase())
        })
        .then_with(|| a.id.cmp(&b.id))
}

fn list_folder_sort_key(list: &ShoppingList) -> (u8, String) {
    if let Some(name) = list
        .folder_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return (0, name.to_ascii_lowercase());
    }

    if let Some(id) = list.folder_id {
        return (0, format!("#{id:020}"));
    }

    (1, String::default())
}

#[cfg(test)]
fn list_has_folder(list: &ShoppingList) -> bool {
    list.folder_id.is_some()
        || list
            .folder_name
            .as_deref()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty())
}

fn list_folder_parts_for_tui(list: &ShoppingList) -> Vec<String> {
    if let Some(folder_name) = list
        .folder_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return folder_name
            .split('/')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect();
    }

    list.folder_id
        .map(|id| vec![format!("#{}", id)])
        .unwrap_or_default()
}

fn list_panel_rows(lists: &[ShoppingList]) -> Vec<ListPanelRow> {
    let mut rows = Vec::new();
    let mut current_folder: Vec<String> = Vec::new();

    for (list_index, list) in lists.iter().enumerate() {
        let folder = list_folder_parts_for_tui(list);
        let common = current_folder
            .iter()
            .zip(folder.iter())
            .take_while(|(a, b)| a.eq_ignore_ascii_case(b))
            .count();
        for (depth, part) in folder.iter().enumerate().skip(common) {
            rows.push(ListPanelRow {
                list_index: None,
                depth,
                label: part.clone(),
            });
        }
        current_folder = folder;
        rows.push(ListPanelRow {
            list_index: Some(list_index),
            depth: current_folder.len(),
            label: list.name.clone(),
        });
    }

    rows
}

fn selected_list_panel_row(rows: &[ListPanelRow], selected_list: usize) -> usize {
    rows.iter()
        .position(|row| row.list_index == Some(selected_list))
        .unwrap_or(0)
}

fn list_display_name_for_tui(list: &ShoppingList) -> String {
    let name = list.name.trim();
    let folder = list
        .folder_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    match (folder, name.is_empty()) {
        (Some(folder_name), false) => format!("{folder_name} / {name}"),
        (Some(folder_name), true) => folder_name.to_string(),
        (None, false) => list.name.clone(),
        (None, true) => tr("common-unknown"),
    }
}

fn trailing_list_icon_marker(
    app: &mut App,
    icon_targets: &mut Vec<(String, Rect)>,
    asset: &str,
    row: u16,
    base_x: u16,
    used_cells: &mut u16,
) -> String {
    let marker = if list_icon_image_enabled(
        app.bootstrap_icons_enabled,
        app.inline_images_enabled,
        app.picker.protocol_type(),
        Some(asset),
    ) {
        app.ensure_list_icon_background(asset);
        icon_targets.push((
            asset.to_string(),
            Rect::new(base_x.saturating_add(*used_cells), row, 2, 1),
        ));
        "   ".to_string()
    } else {
        format!(
            " {}",
            bootstrap_icon_for_tui(&format!("bi-{asset}"), tui_icon_style())
        )
    };
    *used_cells = (*used_cells).saturating_add(marker.chars().count() as u16);
    marker
}

#[cfg(test)]
fn normalize_list_icon(raw_icon: Option<&str>) -> String {
    list_icon_for_tui(raw_icon)
}

fn list_icon_for_tui(raw_icon: Option<&str>) -> String {
    let style = tui_icon_style();
    let icon = list_icon_asset_name(raw_icon);

    if style == TuiIconStyle::Raw {
        let Some(raw_icon) = raw_icon.map(str::trim).filter(|value| !value.is_empty()) else {
            return icon.map_or_else(empty_icon_slot, |icon| {
                bootstrap_icon_for_tui(&format!("bi-{icon}"), style)
            });
        };
        return raw_icon.to_string();
    }

    icon.map_or_else(empty_icon_slot, |icon| {
        bootstrap_icon_for_tui(&format!("bi-{icon}"), style)
    })
}

fn list_icon_asset_name(raw_icon: Option<&str>) -> Option<String> {
    bootstrap_icon_asset_name(raw_icon).or_else(|| Some(DEFAULT_LIST_ICON.to_string()))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TuiIconStyle {
    Label,
    Raw,
}

fn tui_icon_style() -> TuiIconStyle {
    match std::env::var(KRAMLI_ICON_STYLE_ENV)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "label" => TuiIconStyle::Label,
        "raw" => TuiIconStyle::Raw,
        _ => TuiIconStyle::Label,
    }
}

fn empty_icon_slot() -> String {
    "  ".to_string()
}

fn bootstrap_icon_for_tui(icon: &str, style: TuiIconStyle) -> String {
    match style {
        TuiIconStyle::Raw => icon.to_string(),
        TuiIconStyle::Label => format!("[{}]", bootstrap_icon_label(icon)),
    }
}

fn list_icon_images_supported(protocol: ProtocolType) -> bool {
    !matches!(protocol, ProtocolType::Halfblocks)
}

fn list_icon_image_enabled(
    bootstrap_icons_enabled: bool,
    inline_images_enabled: bool,
    protocol: ProtocolType,
    icon_asset: Option<&str>,
) -> bool {
    bootstrap_icons_enabled
        && inline_images_enabled
        && list_icon_images_supported(protocol)
        && icon_asset.is_some()
}

fn bootstrap_icon_asset_name(raw_icon: Option<&str>) -> Option<String> {
    let raw_icon = raw_icon?.trim();
    if raw_icon.is_empty() {
        return None;
    }
    if raw_icon.contains("..") {
        return None;
    }

    if let Some(icon) = extract_bootstrap_class_icon(raw_icon) {
        return Some(icon);
    }

    let candidate = raw_icon
        .trim_matches(|ch: char| ch == '"' || ch == '\'' || ch == '`' || ch == '[' || ch == ']')
        .trim_end_matches(".svg")
        .rsplit(['/', '#', '?'])
        .next()
        .unwrap_or(raw_icon)
        .trim()
        .trim_start_matches("bootstrap-icons:")
        .trim_start_matches("bootstrap-icon:")
        .trim_start_matches("bi:")
        .trim_start_matches("bi_");
    let candidate = candidate
        .strip_prefix("bi-")
        .unwrap_or(candidate)
        .replace('_', "-")
        .to_ascii_lowercase();

    normalize_bootstrap_icon_name(&candidate)
}

fn extract_bootstrap_class_icon(raw_icon: &str) -> Option<String> {
    for (index, _) in raw_icon.match_indices("bi-") {
        let candidate: String = raw_icon[index + 3..]
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
            .collect();
        if let Some(icon) = normalize_bootstrap_icon_name(&candidate.replace('_', "-")) {
            return Some(icon);
        }
    }
    None
}

fn normalize_bootstrap_icon_name(candidate: &str) -> Option<String> {
    let icon = candidate.trim();
    (!icon.is_empty()
        && icon.len() <= 80
        && icon
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-'))
    .then(|| icon.to_string())
}

#[cfg(test)]
fn bootstrap_sprite_contains_icon(sprite: &str, icon: &str) -> bool {
    let Some(icon) = normalize_bootstrap_icon_name(icon) else {
        return false;
    };
    let needle = format!("id=\"{icon}\"");
    sprite.contains(&needle)
}

fn bootstrap_icon_label(icon: &str) -> &str {
    icon.strip_prefix("bi-")
        .filter(|value| !value.is_empty())
        .unwrap_or("list")
}

fn bootstrap_icon_base_url() -> String {
    std::env::var(KRAMLI_BOOTSTRAP_ICON_BASE_URL_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| BOOTSTRAP_ICON_BASE_URL.to_string())
}

async fn fetch_bootstrap_icon_image(icon: &str) -> Result<DynamicImage, String> {
    let Some(icon) = bootstrap_icon_asset_name(Some(icon)) else {
        return Err("invalid bootstrap icon".to_string());
    };
    if let Some(bytes) = read_cached_bootstrap_icon(&icon).await {
        if let Ok(image) = render_bootstrap_svg_icon(&bytes) {
            return Ok(image);
        }
    }

    let url = format!("{}/{}.svg", bootstrap_icon_base_url(), icon);
    let response = reqwest::Client::builder()
        .timeout(Duration::from_secs(6))
        .build()
        .map_err(|error| error.to_string())?
        .get(url)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "bootstrap icon http {}",
            response.status().as_u16()
        ));
    }
    let bytes = response.bytes().await.map_err(|error| error.to_string())?;
    write_cached_bootstrap_icon(&icon, &bytes).await;
    render_bootstrap_svg_icon(&bytes)
}

fn bootstrap_icon_cache_path(icon: &str) -> Option<PathBuf> {
    let icon = normalize_bootstrap_icon_name(icon)?;
    dirs::cache_dir().map(|dir| {
        dir.join("kramli")
            .join("bootstrap-icons")
            .join(format!("{icon}.svg"))
    })
}

async fn read_cached_bootstrap_icon(icon: &str) -> Option<Vec<u8>> {
    let path = bootstrap_icon_cache_path(icon)?;
    tokio::fs::read(path).await.ok()
}

async fn write_cached_bootstrap_icon(icon: &str, bytes: &[u8]) {
    let Some(path) = bootstrap_icon_cache_path(icon) else {
        return;
    };
    if let Some(parent) = path.parent() {
        if tokio::fs::create_dir_all(parent).await.is_err() {
            return;
        }
    }
    let _ = tokio::fs::write(path, bytes).await;
}

fn render_bootstrap_svg_icon(svg: &[u8]) -> Result<DynamicImage, String> {
    render_bootstrap_svg_icon_with_color(svg, &icon_svg_color())
}

fn load_oriented_image(bytes: &[u8]) -> image::ImageResult<DynamicImage> {
    let reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let mut decoder = reader.into_decoder()?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    Ok(image)
}

fn render_bootstrap_svg_icon_with_color(svg: &[u8], color: &str) -> Result<DynamicImage, String> {
    let svg = String::from_utf8_lossy(svg).replace("currentColor", color);
    let options = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(svg.as_bytes(), &options)
        .map_err(|error| error.to_string())?;
    let size = tree.size().to_int_size();
    let scale = 4.0;
    let width = size.width().saturating_mul(4).max(16);
    let height = size.height().saturating_mul(4).max(16);
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| "cannot create icon pixmap".to_string())?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let png = pixmap.encode_png().map_err(|error| error.to_string())?;
    load_from_memory(&png).map_err(|error| error.to_string())
}

fn icon_svg_color() -> String {
    icon_svg_color_from_values(
        std::env::var(KRAMLI_TUI_THEME_ENV).ok().as_deref(),
        std::env::var(COLORFGBG_ENV).ok().as_deref(),
        std::env::var(KRAMLI_TUI_ICON_COLOR_ENV).ok().as_deref(),
    )
}

fn icon_svg_color_from_values(
    theme: Option<&str>,
    colorfgbg: Option<&str>,
    explicit_color: Option<&str>,
) -> String {
    if let Some(color) = explicit_color.and_then(normalize_hex_color) {
        return color;
    }

    match theme.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("light") => return "#1f4f8f".to_string(),
        Some("dark") => return "#7ec8ff".to_string(),
        _ => {}
    }

    if let Some(bg) = colorfgbg.and_then(colorfgbg_background_code) {
        return if terminal_color_code_is_light(bg) {
            "#1f4f8f".to_string()
        } else {
            "#7ec8ff".to_string()
        };
    }

    "#4f7db8".to_string()
}

fn normalize_hex_color(value: &str) -> Option<String> {
    let value = value.trim();
    let hex = value.strip_prefix('#').unwrap_or(value);
    (hex.len() == 6 && hex.chars().all(|ch| ch.is_ascii_hexdigit()))
        .then(|| format!("#{}", hex.to_ascii_lowercase()))
}

fn colorfgbg_background_code(value: &str) -> Option<u16> {
    value
        .split(';')
        .next_back()
        .and_then(|part| part.trim().parse::<u16>().ok())
}

fn terminal_color_code_is_light(code: u16) -> bool {
    matches!(code, 7 | 15) || (8..=15).contains(&code)
}

fn has_image(item: &ListItem) -> bool {
    item.image_url
        .as_deref()
        .is_some_and(|value| !value.is_empty())
        || item
            .attachments
            .as_ref()
            .is_some_and(|attachments| attachments.iter().any(is_image_attachment))
        || item
            .notes
            .as_deref()
            .and_then(extract_note_image_source)
            .is_some()
}

fn is_image_attachment(attachment: &Attachment) -> bool {
    if attachment
        .mime_type
        .as_deref()
        .is_some_and(|value| value.to_ascii_lowercase().starts_with("image/"))
    {
        return true;
    }

    attachment
        .filename
        .as_deref()
        .or(attachment.original_filename.as_deref())
        .is_some_and(|name| {
            let lower = name.to_ascii_lowercase();
            lower.ends_with(".png")
                || lower.ends_with(".jpg")
                || lower.ends_with(".jpeg")
                || lower.ends_with(".webp")
                || lower.ends_with(".gif")
                || lower.ends_with(".bmp")
                || lower.ends_with(".heic")
                || lower.ends_with(".avif")
        })
}

fn extract_note_image_source(notes: &str) -> Option<String> {
    extract_html_image_source(notes).or_else(|| extract_markdown_image_source(notes))
}

fn extract_html_image_source(notes: &str) -> Option<String> {
    let lower = notes.to_ascii_lowercase();
    let mut search_start = 0usize;
    while let Some(relative_pos) = lower[search_start..].find("<img") {
        let tag_start = search_start + relative_pos;
        let tag_end = lower[tag_start..]
            .find('>')
            .map_or(notes.len(), |offset| tag_start + offset);
        let tag = &notes[tag_start..tag_end];
        if let Some(source) = html_attr_value(tag, "src")
            .or_else(|| html_attr_value(tag, "data-src"))
            .and_then(valid_image_source)
        {
            return Some(source);
        }
        search_start = tag_end.saturating_add(1);
    }
    None
}

fn html_attr_value(tag: &str, attr: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut search_start = 0usize;
    let needle = format!("{attr}=");
    while let Some(relative_pos) = lower[search_start..].find(&needle) {
        let start = search_start + relative_pos;
        let before = lower[..start].chars().last();
        if before.is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_') {
            search_start = start.saturating_add(needle.len());
            continue;
        }

        let value_start = start + needle.len();
        let value_bytes = tag.as_bytes();
        if value_start >= value_bytes.len() {
            return None;
        }

        let quote = value_bytes[value_start];
        if quote == b'\'' || quote == b'"' {
            let content_start = value_start + 1;
            let rest = &tag[content_start..];
            let end = rest.find(quote as char)?;
            return Some(rest[..end].to_string());
        }

        let rest = &tag[value_start..];
        let end = match rest.find(|ch: char| ch.is_whitespace() || ch == '>') {
            Some(end) => end,
            None => rest.len(),
        };
        return Some(rest[..end].to_string());
    }
    None
}

fn extract_markdown_image_source(notes: &str) -> Option<String> {
    let mut search_start = 0usize;
    while let Some(relative_pos) = notes[search_start..].find("![") {
        let start = search_start + relative_pos;
        let Some(close_label) = notes[start..].find("](").map(|offset| start + offset) else {
            break;
        };
        let source_start = close_label + 2;
        let Some(close_url) = notes[source_start..]
            .find(')')
            .map(|offset| source_start + offset)
        else {
            break;
        };
        let raw = notes[source_start..close_url]
            .split_whitespace()
            .next()
            .unwrap_or("");
        if let Some(source) = valid_image_source(raw) {
            return Some(source);
        }
        search_start = close_url.saturating_add(1);
    }
    None
}

fn valid_image_source(raw: impl AsRef<str>) -> Option<String> {
    let source = raw
        .as_ref()
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim()
        .to_string();
    if source.is_empty() {
        return None;
    }
    let lower = source.to_ascii_lowercase();
    if lower.starts_with("data:")
        || lower.starts_with("javascript:")
        || lower.starts_with("mailto:")
        || lower.starts_with("tel:")
    {
        return None;
    }
    Some(source)
}

async fn fetch_and_open_image(api: ApiClient, source: String) -> Result<String, String> {
    let bytes = api.get_bytes(&source).await?;
    let path = temp_image_path(&source);
    tokio::fs::write(&path, bytes)
        .await
        .map_err(|error| error.to_string())?;
    open_path(&path).await?;
    Ok(path.display().to_string())
}

fn temp_image_path(source: &str) -> PathBuf {
    let mut hasher = DefaultHasher::default();
    source.hash(&mut hasher);
    let hash = hasher.finish();
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    std::env::temp_dir().join(format!(
        "kramli-cli-image-{millis}-{hash}.{}",
        image_extension_from_source(source)
    ))
}

fn image_extension_from_source(source: &str) -> &'static str {
    let path = source
        .split(['?', '#'])
        .next()
        .unwrap_or(source)
        .to_ascii_lowercase();
    if path.ends_with(".png") {
        "png"
    } else if path.ends_with(".jpeg") {
        "jpeg"
    } else if path.ends_with(".jpg") {
        "jpg"
    } else if path.ends_with(".webp") {
        "webp"
    } else if path.ends_with(".gif") {
        "gif"
    } else if path.ends_with(".bmp") {
        "bmp"
    } else if path.ends_with(".heic") {
        "heic"
    } else if path.ends_with(".avif") {
        "avif"
    } else {
        "jpg"
    }
}

async fn open_path(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = TokioCommand::new("open");
        command.arg(path);
        command
    };

    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = TokioCommand::new("cmd");
        command.args(["/C", "start", ""]);
        command.arg(path);
        command
    };

    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let mut command = {
        let mut command = TokioCommand::new("xdg-open");
        command.arg(path);
        command
    };

    command.spawn().map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ApiClient;

    fn sample_item(id: i64, text: &str) -> ListItem {
        ListItem {
            id,
            list_id: Some(1),
            text: text.to_string(),
            is_done: Some(false),
            quantity: None,
            notes: None,
            tldr: None,
            due_date: None,
            due_time: None,
            reminder: None,
            reminder_time: None,
            reminder_days_before: None,
            reminder_offsets: None,
            travel_time_minutes: None,
            planned_date: None,
            planned_time: None,
            priority: None,
            progress: None,
            tags: None,
            parent_item_id: None,
            depth: None,
            position: None,
            completed_at: None,
            created_at: None,
            updated_at: None,
            assigned_to: None,
            child_count: None,
            done_child_count: None,
            comment_count: None,
            color: None,
            repeat_label: None,
            image_url: None,
            image_filename: None,
            attachments: None,
        }
    }

    fn test_app() -> App {
        App::new(ApiClient::for_tests("https://kramli.test"), true)
    }

    async fn api_with_responses(
        responses: Vec<String>,
    ) -> (ApiClient, tokio::task::JoinHandle<Vec<String>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test server should bind");
        let addr = listener.local_addr().expect("test server should have addr");
        let handle = tokio::spawn(async move {
            let mut requests = Vec::new();
            for body in responses {
                let (mut stream, _) = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    listener.accept(),
                )
                .await
                .expect("test server accept timed out")
                .expect("request should connect");
                let mut buffer = [0_u8; 4096];
                let read = stream.read(&mut buffer).await.expect("request should read");
                let request = String::from_utf8_lossy(&buffer[..read]).to_string();
                requests.push(request.lines().next().unwrap_or_default().to_string());
                let header = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                stream
                    .write_all(header.as_bytes())
                    .await
                    .expect("response header should write");
                stream
                    .write_all(body.as_bytes())
                    .await
                    .expect("response body should write");
            }
            requests
        });

        (ApiClient::for_tests(&format!("http://{addr}")), handle)
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::empty(),
        }
    }

    fn test_list() -> ShoppingList {
        test_shopping_list(1, "Groceries", None, None, None, false)
    }

    fn test_attachment(id: i64, filename: Option<&str>, mime_type: Option<&str>) -> Attachment {
        Attachment {
            id,
            filename: filename.map(str::to_string),
            original_filename: None,
            mime_type: mime_type.map(str::to_string),
            file_size: None,
            url: Some(format!("/uploads/{id}")),
        }
    }

    #[test]
    fn extracts_html_note_image_sources() {
        assert_eq!(
            extract_note_image_source(r#"<p>Text</p><img class="x" src="/uploads/item.jpg">"#),
            Some("/uploads/item.jpg".to_string())
        );
        assert_eq!(
            extract_note_image_source(r#"<img data-src='https://example.test/a.webp'>"#),
            Some("https://example.test/a.webp".to_string())
        );
    }

    #[test]
    fn extracts_markdown_note_image_sources() {
        assert_eq!(
            extract_note_image_source("before ![photo](https://example.test/p.png) after"),
            Some("https://example.test/p.png".to_string())
        );
    }

    #[test]
    fn rejects_data_image_sources_for_external_opening() {
        assert_eq!(
            extract_note_image_source(r#"<img src="data:image/png;base64,abc">"#),
            None
        );
    }

    #[test]
    fn image_source_helpers_cover_scheme_extension_and_temp_paths() {
        assert_eq!(valid_image_source(" javascript:alert(1) "), None);
        assert_eq!(valid_image_source("mailto:test@example.com"), None);
        assert_eq!(valid_image_source("tel:+410000000"), None);
        assert_eq!(
            valid_image_source(" 'https://example.test/photo.jpeg?size=large' "),
            Some("https://example.test/photo.jpeg?size=large".to_string())
        );
        assert_eq!(valid_image_source("  "), None);

        assert_eq!(image_extension_from_source("/a/photo.png?x=1"), "png");
        assert_eq!(image_extension_from_source("/a/photo.jpeg#frag"), "jpeg");
        assert_eq!(image_extension_from_source("/a/photo.jpg"), "jpg");
        assert_eq!(image_extension_from_source("/a/photo.webp"), "webp");
        assert_eq!(image_extension_from_source("/a/photo.gif"), "gif");
        assert_eq!(image_extension_from_source("/a/photo.bmp"), "bmp");
        assert_eq!(image_extension_from_source("/a/photo.heic"), "heic");
        assert_eq!(image_extension_from_source("/a/photo.avif"), "avif");
        assert_eq!(image_extension_from_source("/a/photo"), "jpg");

        let path = temp_image_path("https://example.test/photo.png?x=1");
        assert_eq!(
            path.extension().and_then(|value| value.to_str()),
            Some("png")
        );
        assert!(path
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|name| name.starts_with("kramli-cli-image-")));
    }

    #[test]
    fn image_source_helpers_cover_attachment_and_parser_edge_cases() {
        let mut attachment = test_attachment(1, None, Some("image/png"));
        assert!(is_image_attachment(&attachment));

        attachment.mime_type = None;
        attachment.filename = Some("photo.AVIF".to_string());
        assert!(is_image_attachment(&attachment));

        attachment.filename = Some("notes.txt".to_string());
        assert!(!is_image_attachment(&attachment));

        assert_eq!(
            html_attr_value(r#"<img src=https://example.test/plain.png>"#, "src"),
            Some("https://example.test/plain.png".to_string())
        );
        assert_eq!(html_attr_value("src=", "src"), None);

        assert_eq!(extract_markdown_image_source("![broken"), None);
        assert_eq!(
            extract_markdown_image_source("![alt](https://example.test/photo.png"),
            None
        );
        assert_eq!(
            extract_markdown_image_source(
                "![bad](javascript:alert) and ![ok](https://example.test/photo.png)",
            ),
            Some("https://example.test/photo.png".to_string())
        );
    }

    #[test]
    fn item_display_helpers_cover_dates_reminders_and_filters() {
        assert_eq!(date_with_time_display(None, Some("08:00")), "-");
        assert_eq!(
            date_with_time_display(Some("2026-01-02T09:10:11Z"), None),
            "2026-01-02"
        );
        assert_eq!(
            date_with_time_display(Some("2026-01-02"), Some(" 08:00 ")),
            "2026-01-02 08:00"
        );
        assert_eq!(
            reminder_offsets_display(&[30, 60, 120, 1440, 2880]),
            "30m, 1h, 2h, 1d, 2d"
        );

        let mut item = sample_item(1, "Buy milk");
        item.quantity = Some("2 cartons".to_string());
        item.notes = Some("<p>Remember bread</p>".to_string());
        item.priority = Some("High".to_string());
        item.tags = Some(vec!["Dairy".to_string(), "Weekend".to_string()]);

        assert!(item_matches_filter(&item, "milk"));
        assert!(item_matches_filter(&item, "cartons"));
        assert!(item_matches_filter(&item, "bread"));
        assert!(item_matches_filter(&item, "high"));
        assert!(item_matches_filter(&item, "weekend"));
        assert!(!item_matches_filter(&item, "hardware"));
    }

    #[test]
    fn render_helpers_cover_list_detail_footer_and_consent_overlays() {
        let mut app = test_app();
        app.beta_consent_pending = false;
        app.profile_name = Some("Ada Lovelace".to_string());
        app.image_runtime_info = Some("images: text".to_string());
        app.image_runtime_debug = vec!["probe: off".to_string()];
        app.status = Some("ready".to_string());
        app.focus = FocusPane::Items;
        app.lists = vec![test_shopping_list(
            1,
            "Groceries",
            Some("bi-cart-fill"),
            Some(9),
            Some("Home / Weekly"),
            true,
        )];
        app.selected_list = 0;

        let mut item = sample_item(1, "Buy milk");
        item.quantity = Some("2 cartons".to_string());
        item.due_date = Some("2026-01-02".to_string());
        item.due_time = Some("08:30".to_string());
        item.planned_date = Some("2026-01-01".to_string());
        item.planned_time = Some("18:00".to_string());
        item.repeat_label = Some("weekly".to_string());
        item.reminder = Some(true);
        item.reminder_time = Some("07:30".to_string());
        item.reminder_offsets = Some(vec![30, 60, 1440]);
        item.travel_time_minutes = Some(15);
        item.priority = Some("high".to_string());
        item.tags = Some(vec!["dairy".to_string()]);
        item.notes = Some("<p>Cold aisle</p>".to_string());
        item.comment_count = Some(2);
        item.image_url = Some("https://example.test/milk.jpg".to_string());
        app.items = vec![item];
        app.comments_cache.insert(
            1,
            vec![ItemComment {
                id: 1,
                text: Some("Fresh".to_string()),
                user_id: Some(2),
                user_name: Some("Grace".to_string()),
                user_email: None,
                created_at: Some("2026-01-01".to_string()),
            }],
        );
        app.detail_image_note = Some("inline images disabled".to_string());

        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
        terminal.draw(|frame| draw_ui(frame, &mut app)).unwrap();

        app.beta_consent_pending = true;
        terminal.draw(|frame| draw_ui(frame, &mut app)).unwrap();

        app.beta_consent_pending = false;
        app.legal_consent_pending = true;
        app.legal_pending_docs = vec!["agb".to_string(), "privacy".to_string()];
        terminal.draw(|frame| draw_ui(frame, &mut app)).unwrap();
    }

    #[test]
    fn render_helpers_cover_image_editor_help_and_mode_branches() {
        let mut app = test_app();
        app.beta_consent_pending = false;
        app.profile_name = Some("Ada Lovelace".to_string());
        app.lists = vec![test_shopping_list(
            1,
            "Groceries",
            Some("bi-cart-fill"),
            Some(9),
            Some("Home / Weekly"),
            true,
        )];
        app.selected_list = 0;

        let image_source = "https://example.test/milk.jpg".to_string();
        let mut item = sample_item(1, "Buy milk");
        item.image_url = Some(image_source.clone());
        app.items = vec![item];
        app.detail_image = Some(DetailImageState {
            source: image_source,
            protocol: app
                .picker
                .new_resize_protocol(DynamicImage::new_rgba8(2, 2)),
        });
        app.profile_image = Some(DetailImageState {
            source: "https://example.test/profile.png".to_string(),
            protocol: app
                .picker
                .new_resize_protocol(DynamicImage::new_rgba8(2, 2)),
        });
        app.show_help = true;
        app.open_filter_editor().expect("filter editor should open");

        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
        terminal.draw(|frame| draw_ui(frame, &mut app)).unwrap();

        app.show_help = false;
        app.editor = None;
        app.mode = ViewMode::Kanban;
        terminal.draw(|frame| draw_ui(frame, &mut app)).unwrap();

        app.mode = ViewMode::Calendar;
        terminal.draw(|frame| draw_ui(frame, &mut app)).unwrap();

        app.legal_consent_pending = true;
        app.legal_accepting = true;
        app.legal_pending_docs.clear();
        terminal.draw(|frame| draw_ui(frame, &mut app)).unwrap();
    }

    #[test]
    fn render_helpers_cover_kanban_calendar_drag_and_editor_variants() {
        let mut app = test_app();
        app.beta_consent_pending = false;
        app.lists = vec![test_list()];
        app.lists[0].states = Some(vec![
            ApiListState {
                name: Some("Inbox".to_string()),
                color: None,
                is_done: Some(false),
            },
            ApiListState {
                name: Some("Doing".to_string()),
                color: None,
                is_done: Some(false),
            },
            ApiListState {
                name: Some("Review".to_string()),
                color: None,
                is_done: Some(false),
            },
            ApiListState {
                name: Some("Blocked".to_string()),
                color: None,
                is_done: Some(false),
            },
            ApiListState {
                name: Some("Done".to_string()),
                color: None,
                is_done: Some(true),
            },
        ]);
        app.items = (0..24)
            .map(|index| {
                let mut item = sample_item(index + 1, &format!("Task {index}"));
                item.progress = Some(if index % 2 == 0 {
                    "Review".to_string()
                } else {
                    "Doing".to_string()
                });
                item
            })
            .collect();
        app.selected_item = 18;
        app.mode = ViewMode::Kanban;
        app.focus = FocusPane::Items;
        app.kanban_drag_item = Some(3);
        app.kanban_drag_started = true;
        app.kanban_drag_target_column = Some(2);

        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(48, 12)).unwrap();
        terminal.draw(|frame| draw_ui(frame, &mut app)).unwrap();

        app.mode = ViewMode::Calendar;
        app.calendar_selected_date = Some(SimpleDate {
            year: 2026,
            month: 7,
            day: 15,
        });
        app.calendar_drag_item = Some(0);
        app.calendar_drag_started = true;
        app.calendar_drag_target_date = Some(SimpleDate {
            year: 2026,
            month: 7,
            day: 16,
        });
        terminal.draw(|frame| draw_ui(frame, &mut app)).unwrap();

        app.calendar_drag_target_date = None;
        terminal.draw(|frame| draw_ui(frame, &mut app)).unwrap();

        fn editor_for_mode(mode: EditorMode) -> EditorState {
            EditorState {
                mode,
                item_id: Some(1),
                text: "Alpha".to_string(),
                quantity: "2".to_string(),
                due_date: "2026-07-15".to_string(),
                due_time: "09:00".to_string(),
                planned_date: "2026-07-14".to_string(),
                planned_time: "18:00".to_string(),
                reminder: "on".to_string(),
                reminder_time: "08:30".to_string(),
                reminder_offsets: "30".to_string(),
                travel_time_minutes: "15".to_string(),
                priority: "high".to_string(),
                tags: "x,y".to_string(),
                progress: "Review".to_string(),
                notes: "note".to_string(),
                active_field: EditorField::Progress,
            }
        }

        app.mode = ViewMode::List;
        app.calendar_drag_item = None;
        app.calendar_drag_started = false;
        app.editor = Some(editor_for_mode(EditorMode::Create));
        terminal.draw(|frame| draw_ui(frame, &mut app)).unwrap();

        app.editor = Some(editor_for_mode(EditorMode::Edit));
        terminal.draw(|frame| draw_ui(frame, &mut app)).unwrap();

        let mut comment_editor = editor_for_mode(EditorMode::Comment);
        comment_editor.active_field = EditorField::Text;
        app.editor = Some(comment_editor);
        terminal.draw(|frame| draw_ui(frame, &mut app)).unwrap();
    }

    #[test]
    fn footer_action_metadata_covers_shortcuts_labels_and_env_names() {
        let actions = [
            FooterAction::Add,
            FooterAction::Refresh,
            FooterAction::Filter,
            FooterAction::Edit,
            FooterAction::ToggleDone,
            FooterAction::Delete,
            FooterAction::OpenImage,
            FooterAction::Comment,
            FooterAction::Undo,
            FooterAction::Members,
            FooterAction::Invite,
            FooterAction::Help,
            FooterAction::Quit,
        ];
        for action in actions {
            assert!(!action.chip_shortcut().is_empty());
            assert!(!action.chip_label().is_empty());
            assert!(action.key_env_name().starts_with("KRAMLI_TUI_KEY_"));
        }
    }

    #[tokio::test]
    async fn editor_save_helpers_cover_progress_and_reminder_body_paths() {
        let (api, requests) = api_with_responses(vec![
            serde_json::json!({
                "id": 1,
                "list_id": 1,
                "text": "Edited",
                "is_done": false,
                "progress": "CustomState"
            })
            .to_string(),
            serde_json::json!({
                "id": 2,
                "list_id": 1,
                "text": "Created",
                "is_done": false,
                "progress": "Open"
            })
            .to_string(),
        ])
        .await;
        let mut app = App::new(api, true);
        app.lists = vec![test_list()];
        app.items = vec![sample_item(1, "Original")];
        app.items[0].progress = Some("CustomState".to_string());
        app.comments_cache.insert(1, Vec::new());
        app.comments_cache.insert(2, Vec::new());

        app.open_comment_editor().unwrap();
        assert_eq!(app.editor.as_ref().map(|editor| editor.mode), Some(EditorMode::Comment));
        app.editor = None;

        app.editor = Some(EditorState {
            mode: EditorMode::Edit,
            item_id: Some(1),
            text: "Edited".to_string(),
            quantity: "2".to_string(),
            due_date: "2026-08-01".to_string(),
            due_time: "08:00".to_string(),
            planned_date: "2026-07-31".to_string(),
            planned_time: "19:00".to_string(),
            reminder: String::default(),
            reminder_time: "07:30".to_string(),
            reminder_offsets: "15, 60".to_string(),
            travel_time_minutes: "20".to_string(),
            priority: "high".to_string(),
            tags: "x, y".to_string(),
            progress: "customstate".to_string(),
            notes: "memo".to_string(),
            active_field: EditorField::Text,
        });
        app.save_editor().await.unwrap();

        app.editor = Some(EditorState {
            mode: EditorMode::Create,
            item_id: None,
            text: "Created".to_string(),
            quantity: String::default(),
            due_date: String::default(),
            due_time: String::default(),
            planned_date: String::default(),
            planned_time: String::default(),
            reminder: "off".to_string(),
            reminder_time: String::default(),
            reminder_offsets: String::default(),
            travel_time_minutes: String::default(),
            priority: String::default(),
            tags: String::default(),
            progress: app.default_progress_value(),
            notes: String::default(),
            active_field: EditorField::Text,
        });
        app.save_editor().await.unwrap();

        let requests = requests.await.expect("test server should finish");
        assert_eq!(
            requests
                .iter()
                .map(|request| request.lines().next().unwrap_or_default().to_string())
                .collect::<Vec<_>>(),
            vec![
                "PUT /api/items/1 HTTP/1.1",
                "POST /api/lists/1/items HTTP/1.1"
            ]
        );
        assert_eq!(app.items.len(), 2);
        assert_eq!(app.selected_item, 1);
    }

    #[tokio::test]
    async fn move_helpers_cover_kanban_and_calendar_update_paths() {
        let (api, requests) = api_with_responses(vec![
            serde_json::json!({
                "id": 1,
                "list_id": 1,
                "text": "Alpha",
                "is_done": false,
                "progress": "Done"
            })
            .to_string(),
            serde_json::json!({
                "id": 1,
                "list_id": 1,
                "text": "Alpha",
                "is_done": true,
                "progress": "Done"
            })
            .to_string(),
            serde_json::json!({
                "id": 1,
                "list_id": 1,
                "text": "Alpha",
                "is_done": true,
                "progress": "Done",
                "due_date": "2026-09-10"
            })
            .to_string(),
        ])
        .await;
        let mut app = App::new(api, true);
        app.mode = ViewMode::List;
        app.lists = vec![test_list()];
        app.lists[0].states = Some(vec![
            ApiListState {
                name: Some("Inbox".to_string()),
                color: None,
                is_done: Some(false),
            },
            ApiListState {
                name: Some("Done".to_string()),
                color: None,
                is_done: Some(true),
            },
        ]);
        app.items = vec![sample_item(1, "Alpha")];
        app.items[0].progress = Some("Inbox".to_string());
        app.items[0].is_done = Some(false);
        app.comments_cache.insert(1, Vec::new());

        app.move_item_to_kanban_column(0, 1).await.unwrap();
        assert_eq!(app.items[0].is_done, Some(true));
        assert_eq!(app.items[0].progress.as_deref(), Some("Done"));

        app.update_item_due_date(0, "2026-09-10".to_string())
            .await
            .unwrap();
        assert_eq!(app.calendar_selected_date.map(|date| date.day), Some(10));
        assert!(app.calendar_visible_month.is_some());

        let requests = requests.await.expect("test server should finish");
        assert_eq!(
            requests
                .iter()
                .map(|request| request.lines().next().unwrap_or_default().to_string())
                .collect::<Vec<_>>(),
            vec![
                "PUT /api/items/1 HTTP/1.1",
                "PATCH /api/items/1/done HTTP/1.1",
                "PUT /api/items/1 HTTP/1.1"
            ]
        );
    }

    #[tokio::test]
    async fn image_background_helpers_cover_spawn_and_error_paths() {
        let mut app = test_app();
        app.lists = vec![test_list()];
        let mut item = sample_item(1, "Image item");
        item.image_url = Some("::bad-source".to_string());
        app.items = vec![item];
        app.selected_item = 0;

        app.set_inline_images_enabled(true);
        app.refresh_selected_image_background();
        assert_eq!(app.pending_detail_image.as_deref(), Some("::bad-source"));
        assert!(app.detail_image.is_none());

        app.profile_photo_url = Some("::bad-profile".to_string());
        app.refresh_profile_image_background();
        assert_eq!(app.pending_profile_image.as_deref(), Some("::bad-profile"));
        assert!(app.profile_image.is_none());

        app.set_inline_images_enabled(false);
        assert!(app.pending_detail_image.is_none());
        assert!(app.pending_profile_image.is_none());

        app.set_inline_images_enabled(true);
        app.pending_open_image = Some("::bad-source".to_string());
        app.apply_open_image_result("::bad-source".to_string(), Err("open failed".to_string()));
        assert!(app.pending_open_image.is_none());
        assert_eq!(app.status.as_deref(), Some("open failed"));

        app.set_inline_images_enabled(false);
        app.open_selected_image_background().unwrap();
        assert_eq!(app.pending_open_image.as_deref(), Some("::bad-source"));
        assert!(app.status.is_some());
    }

    #[tokio::test]
    async fn profile_and_image_background_loaders_cover_success_paths() {
        let (api, requests) = api_with_responses(vec![
            serde_json::json!({"display_name": "Ada"}).to_string(),
            "detail-bytes".to_string(),
            "profile-bytes".to_string(),
        ])
        .await;

        let mut app = App::new(api, true);
        app.load_profile_background();

        for _ in 0..120 {
            let _ = app.drain_load_messages();
            if app.profile_name.as_deref() == Some("Ada") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert_eq!(app.profile_name.as_deref(), Some("Ada"));

        app.set_inline_images_enabled(true);
        app.lists = vec![test_list()];
        let mut item = sample_item(1, "Image item");
        item.image_url = Some("/img/detail.png".to_string());
        app.items = vec![item];
        app.selected_item = 0;
        app.profile_photo_url = Some("/img/profile.png".to_string());

        app.refresh_selected_image_background();
        app.refresh_profile_image_background();

        for _ in 0..120 {
            let _ = app.drain_load_messages();
            if app.pending_detail_image.is_none() && app.pending_profile_image.is_none() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }

        assert!(app.pending_detail_image.is_none());
        assert!(app.pending_profile_image.is_none());

        let requests = requests.await.expect("test server should finish");
        assert_eq!(
            requests,
            vec![
                "GET /api/profile HTTP/1.1",
                "GET /img/detail.png HTTP/1.1",
                "GET /img/profile.png HTTP/1.1"
            ]
        );
    }

    #[tokio::test]
    async fn open_external_image_loader_path_is_reachable() {
        let (api, requests) = api_with_responses(vec!["bytes".to_string()]).await;
        let mut app = App::new(api, true);
        app.lists = vec![test_list()];
        let mut item = sample_item(1, "Open image");
        item.image_url = Some("/img/open.png".to_string());
        app.items = vec![item];
        app.selected_item = 0;

        app.open_selected_image_background()
            .expect("open image background should queue a load message");
        assert_eq!(app.pending_open_image.as_deref(), Some("/img/open.png"));

        for _ in 0..200 {
            let _ = app.drain_load_messages();
            if app.pending_open_image.is_none() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }

        assert!(app.pending_open_image.is_none());

        let requests = requests.await.expect("test server should finish");
        assert_eq!(requests, vec!["GET /img/open.png HTTP/1.1"]);
    }

    #[tokio::test]
    async fn event_loop_and_runtime_event_helpers_cover_input_branches() {
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        let mut app = test_app();
        app.should_quit = true;
        run_event_loop(&mut terminal, &mut app)
            .await
            .expect("event loop should exit immediately when quit is already set");

        let mut app = test_app();
        let release_key = KeyEvent {
            code: KeyCode::Char('x'),
            modifiers: KeyModifiers::empty(),
            kind: KeyEventKind::Release,
            state: crossterm::event::KeyEventState::empty(),
        };
        assert!(
            !handle_runtime_event(&mut terminal, &mut app, Event::Key(release_key))
                .await
                .expect("release key should be ignored")
        );

        assert!(
            handle_runtime_event(
                &mut terminal,
                &mut app,
                Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            )
            .await
            .expect("global quit key should be handled")
        );
        assert!(app.should_quit);

        app.should_quit = false;
        app.beta_consent_pending = true;
        assert!(
            handle_runtime_event(
                &mut terminal,
                &mut app,
                Event::Mouse(mouse(MouseEventKind::Down(MouseButton::Left), 2, 2)),
            )
            .await
            .expect("beta consent mouse event should be handled")
        );

        app.beta_consent_pending = false;
        app.editor = Some(EditorState {
            mode: EditorMode::Create,
            item_id: None,
            text: String::default(),
            quantity: String::default(),
            due_date: String::default(),
            due_time: String::default(),
            planned_date: String::default(),
            planned_time: String::default(),
            reminder: String::default(),
            reminder_time: String::default(),
            reminder_offsets: String::default(),
            travel_time_minutes: String::default(),
            priority: String::default(),
            tags: String::default(),
            progress: String::default(),
            notes: String::default(),
            active_field: EditorField::Text,
        });
        assert!(
            handle_runtime_event(
                &mut terminal,
                &mut app,
                Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::empty())),
            )
            .await
            .expect("editor key event should be handled")
        );
        assert!(
            handle_runtime_event(
                &mut terminal,
                &mut app,
                Event::Mouse(mouse(MouseEventKind::Down(MouseButton::Left), 0, 0)),
            )
            .await
            .expect("editor mouse event should be handled")
        );

        assert!(
            handle_runtime_event(&mut terminal, &mut app, Event::Resize(100, 40))
                .await
                .expect("resize event should be handled")
        );
        assert!(
            !handle_runtime_event(&mut terminal, &mut app, Event::FocusGained)
                .await
                .expect("focus-gained event should be ignored")
        );
    }

    #[tokio::test]
    async fn run_tui_session_helper_covers_success_and_restore_error_paths() {
        crate::test_env::with_env_lock_async(|| async {
            let previous_protocol = std::env::var_os(KRAMLI_TUI_IMAGE_PROTOCOL_ENV);

            std::env::set_var(KRAMLI_TUI_IMAGE_PROTOCOL_ENV, "off");
            let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
            run_tui_session(
                &mut terminal,
                ApiClient::for_tests("https://kramli.test"),
                true,
                |_| Ok(()),
                |app| {
                    app.should_quit = true;
                },
            )
            .await
            .expect("session helper should succeed when restore succeeds");

            std::env::set_var(KRAMLI_TUI_IMAGE_PROTOCOL_ENV, "auto");
            let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
            let result = run_tui_session(
                &mut terminal,
                ApiClient::for_tests("https://kramli.test"),
                true,
                |_| Err("restore failed".to_string()),
                |app| {
                    app.should_quit = true;
                },
            )
            .await;
            assert!(result.is_err());

            match previous_protocol {
                Some(value) => std::env::set_var(KRAMLI_TUI_IMAGE_PROTOCOL_ENV, value),
                None => std::env::remove_var(KRAMLI_TUI_IMAGE_PROTOCOL_ENV),
            }
        })
        .await;
    }

    #[tokio::test]
    async fn run_tui_terminal_factory_covers_init_error_and_success_paths() {
        let err = run_tui_with_terminal_factory::<ratatui::backend::TestBackend, _, _, _>(
            ApiClient::for_tests("https://kramli.test"),
            true,
            || Err("init failed".to_string()),
            |_| Ok(()),
            |_| {},
        )
        .await;
        assert!(err.is_err());

        run_tui_with_terminal_factory(
            ApiClient::for_tests("https://kramli.test"),
            true,
            || {
                Terminal::new(ratatui::backend::TestBackend::new(80, 24))
                    .map_err(|error| error.to_string())
            },
            |_| Ok(()),
            |app| {
                app.should_quit = true;
            },
        )
        .await
        .expect("terminal factory helper should succeed with a test backend");
    }

    #[test]
    fn restore_terminal_function_is_reachable() {
        let backend = CrosstermBackend::new(io::stdout());
        let mut terminal = Terminal::new(backend).expect("stdout terminal should construct");
        let _ = restore_terminal(&mut terminal);
    }

    #[tokio::test]
    async fn calendar_drag_helpers_cover_started_and_date_click_paths() {
        let responses = vec![
            serde_json::json!({"id": 1, "list_id": 1, "text": "Milk", "is_done": false, "due_date": "2026-07-11"})
                .to_string(),
            serde_json::json!({"id": 1, "list_id": 1, "text": "Milk", "is_done": false, "due_date": "2026-07-12"})
                .to_string(),
        ];
        let (api, requests) = api_with_responses(responses).await;
        let mut app = App::new(api, true);
        app.lists = vec![test_list()];
        let mut item = sample_item(1, "Milk");
        item.due_date = Some("2026-07-10".to_string());
        app.items = vec![item];
        app.selected_item = 0;

        app.start_calendar_item_drag(0);
        assert_eq!(app.calendar_drag_item, Some(0));

        app.calendar_drag_item = Some(0);
        app.calendar_drag_source_date = Some(SimpleDate {
            year: 2026,
            month: 7,
            day: 10,
        });
        app.calendar_drag_started = true;
        app.calendar_drag_target_date = Some(SimpleDate {
            year: 2026,
            month: 7,
            day: 11,
        });
        app.finish_started_calendar_drag(
            Rect::new(0, 0, 60, 20),
            mouse(MouseEventKind::Up(MouseButton::Left), 0, 0),
        )
        .await
        .expect("started drag should move item when target date differs");
        assert_eq!(app.calendar_selected_date.map(|date| date.day), Some(11));

        app.calendar_drag_item = Some(0);
        app.calendar_drag_started = false;
        app.finish_calendar_drag(
            Rect::new(0, 0, 60, 20),
            mouse(MouseEventKind::Up(MouseButton::Left), 0, 0),
        )
        .await
        .expect("non-started drag should only update status");
        assert_eq!(app.status.as_deref(), Some(tr("tui-help-calendar-3").as_str()));

        app.calendar_drag_item = Some(0);
        app.calendar_drag_source_date = Some(SimpleDate {
            year: 2026,
            month: 7,
            day: 11,
        });
        app.handle_calendar_date_click(SimpleDate {
            year: 2026,
            month: 7,
            day: 12,
        })
        .await
        .expect("calendar date click should move dragged item");
        assert_eq!(app.calendar_selected_date.map(|date| date.day), Some(12));

        app.handle_calendar_date_click(SimpleDate {
            year: 2026,
            month: 7,
            day: 12,
        })
        .await
        .expect("calendar date click should toggle selected date");
        assert!(app.calendar_selected_date.is_none());

        let requests = requests.await.expect("test server should finish");
        assert_eq!(
            requests,
            vec!["PUT /api/items/1 HTTP/1.1", "PUT /api/items/1 HTTP/1.1"]
        );
    }

    #[tokio::test]
    async fn accept_beta_consent_reloads_lists_when_legal_is_clear() {
        let (api, requests) = api_with_responses(vec![serde_json::json!([]).to_string()]).await;
        let mut app = App::new(api, true);
        app.beta_consent_pending = true;
        app.legal_consent_pending = false;
        app.initial_load_started = true;

        app.accept_beta_consent();

        for _ in 0..80 {
            let _ = app.drain_load_messages();
            if !app.loading_lists {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }

        assert!(!app.beta_consent_pending);
        let requests = requests.await.expect("test server should finish");
        assert_eq!(requests, vec!["GET /api/lists HTTP/1.1"]);
    }

    #[test]
    fn draw_list_panel_uses_image_icon_targets_for_folder_and_lists() {
        let mut app = test_app();
        app.set_inline_images_enabled(true);
        app.picker.set_protocol_type(ProtocolType::Sixel);
        app.failed_list_icons.insert("folder2".to_string());
        app.failed_list_icons.insert("cart-fill".to_string());
        app.lists = vec![test_shopping_list(
            1,
            "Groceries",
            Some("bi-cart-fill"),
            Some(6),
            Some("Home / Weekly"),
            false,
        )];

        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
        terminal.draw(|frame| draw_ui(frame, &mut app)).unwrap();
    }

    #[tokio::test]
    async fn ensure_list_icon_background_queues_and_applies_error_results() {
        let mut app = test_app();
        app.set_inline_images_enabled(true);
        app.ensure_list_icon_background("bad..icon");
        assert!(app.pending_list_icons.contains("bad..icon"));

        for _ in 0..80 {
            let _ = app.drain_load_messages();
            if !app.pending_list_icons.contains("bad..icon") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }

        assert!(!app.pending_list_icons.contains("bad..icon"));
        assert!(app.failed_list_icons.contains("bad..icon"));
    }

    #[tokio::test]
    async fn bootstrap_icon_fetch_uses_override_base_url_and_handles_http_errors() {
        crate::test_env::with_env_lock_async(|| async {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};

            async fn icon_server(
                status: u16,
                body: &str,
            ) -> (String, tokio::task::JoinHandle<Vec<String>>) {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                    .await
                    .expect("test server should bind");
                let addr = listener.local_addr().expect("test server should have addr");
                let body = body.to_string();
                let handle = tokio::spawn(async move {
                    let mut requests = Vec::new();
                    let (mut stream, _) = tokio::time::timeout(
                        std::time::Duration::from_secs(5),
                        listener.accept(),
                    )
                    .await
                    .expect("test server accept timed out")
                    .expect("request should connect");
                    let mut buffer = [0_u8; 4096];
                    let read = stream.read(&mut buffer).await.expect("request should read");
                    let request = String::from_utf8_lossy(&buffer[..read]).to_string();
                    requests.push(request.lines().next().unwrap_or_default().to_string());
                    let header = format!(
                        "HTTP/1.1 {status} TEST\r\ncontent-type: image/svg+xml\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        body.len()
                    );
                    stream
                        .write_all(header.as_bytes())
                        .await
                        .expect("response header should write");
                    stream
                        .write_all(body.as_bytes())
                        .await
                        .expect("response body should write");
                    requests
                });
                (format!("http://{addr}"), handle)
            }

            let svg = r#"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><path fill='currentColor' d='M0 0h16v16H0z'/></svg>"#;
            let (ok_url, ok_requests) = icon_server(200, svg).await;
            std::env::set_var(KRAMLI_BOOTSTRAP_ICON_BASE_URL_ENV, ok_url);
            let icon_name = format!("kramli-test-{}", std::process::id());
            fetch_bootstrap_icon_image(&icon_name)
                .await
                .expect("icon fetch should render SVG with an override base URL");
            let ok_requests = ok_requests.await.expect("test server should finish");
            assert_eq!(
                ok_requests,
                vec![format!("GET /{icon_name}.svg HTTP/1.1")]
            );

            let (err_url, err_requests) = icon_server(404, "{}").await;
            std::env::set_var(KRAMLI_BOOTSTRAP_ICON_BASE_URL_ENV, err_url);
            let err = fetch_bootstrap_icon_image("kramli-test-missing")
                .await
                .expect_err("non-success icon status should return an error");
            assert!(err.contains("bootstrap icon http 404"));
            let err_requests = err_requests.await.expect("test server should finish");
            assert_eq!(
                err_requests,
                vec!["GET /kramli-test-missing.svg HTTP/1.1"]
            );

            std::env::remove_var(KRAMLI_BOOTSTRAP_ICON_BASE_URL_ENV);
        })
        .await;
    }

    #[test]
    fn beta_overlay_renders_profile_image_when_available() {
        let mut app = test_app();
        app.beta_consent_pending = true;
        app.profile_name = Some("Ada".to_string());
        app.profile_image = Some(DetailImageState {
            source: "https://example.test/p.png".to_string(),
            protocol: app
                .picker
                .new_resize_protocol(DynamicImage::new_rgba8(2, 2)),
        });

        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
        terminal.draw(|frame| draw_ui(frame, &mut app)).unwrap();
    }

    #[test]
    fn draw_editor_and_protocol_fallback_helpers_cover_remaining_paths() {
        crate::test_env::with_env_lock(|| {
            let previous_term = std::env::var_os(TERM_ENV);
            let previous_program = std::env::var_os(TERM_PROGRAM_ENV);
            let previous_lc_terminal = std::env::var_os(LC_TERMINAL_ENV);
            let previous_kitty = std::env::var_os(KITTY_WINDOW_ID_ENV);
            let previous_iterm = std::env::var_os(ITERM_SESSION_ID_ENV);
            let previous_wt = std::env::var_os(WT_SESSION_ENV);

            std::env::set_var(TERM_ENV, "xterm-kitty");
            std::env::set_var(KITTY_WINDOW_ID_ENV, "1");
            assert_eq!(autodetect_protocol_fallback(), Some(ProtocolType::Kitty));
            std::env::remove_var(KITTY_WINDOW_ID_ENV);
            std::env::set_var(TERM_ENV, "xterm-256color");
            std::env::set_var(TERM_PROGRAM_ENV, "iTerm.app");
            std::env::set_var(LC_TERMINAL_ENV, "iTerm2");
            std::env::set_var(ITERM_SESSION_ID_ENV, "abc");
            assert_eq!(autodetect_protocol_fallback(), Some(ProtocolType::Iterm2));
            std::env::set_var(TERM_PROGRAM_ENV, "Windows_Terminal");
            std::env::set_var(WT_SESSION_ENV, "xyz");
            assert_eq!(autodetect_protocol_fallback(), Some(ProtocolType::Sixel));

            match previous_term {
                Some(value) => std::env::set_var(TERM_ENV, value),
                None => std::env::remove_var(TERM_ENV),
            }
            match previous_program {
                Some(value) => std::env::set_var(TERM_PROGRAM_ENV, value),
                None => std::env::remove_var(TERM_PROGRAM_ENV),
            }
            match previous_lc_terminal {
                Some(value) => std::env::set_var(LC_TERMINAL_ENV, value),
                None => std::env::remove_var(LC_TERMINAL_ENV),
            }
            match previous_kitty {
                Some(value) => std::env::set_var(KITTY_WINDOW_ID_ENV, value),
                None => std::env::remove_var(KITTY_WINDOW_ID_ENV),
            }
            match previous_iterm {
                Some(value) => std::env::set_var(ITERM_SESSION_ID_ENV, value),
                None => std::env::remove_var(ITERM_SESSION_ID_ENV),
            }
            match previous_wt {
                Some(value) => std::env::set_var(WT_SESSION_ENV, value),
                None => std::env::remove_var(WT_SESSION_ENV),
            }

            let mut editor = EditorState {
                mode: EditorMode::Edit,
                item_id: Some(1),
                text: "Text".to_string(),
                quantity: "1".to_string(),
                due_date: "2026-10-01".to_string(),
                due_time: "08:00".to_string(),
                planned_date: "2026-09-30".to_string(),
                planned_time: "19:00".to_string(),
                reminder: "on".to_string(),
                reminder_time: "07:30".to_string(),
                reminder_offsets: "30".to_string(),
                travel_time_minutes: "10".to_string(),
                priority: "high".to_string(),
                tags: "a,b".to_string(),
                progress: "Open".to_string(),
                notes: "notes".to_string(),
                active_field: EditorField::Text,
            };

            let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 20)).unwrap();
            for field in ITEM_EDITOR_FIELDS {
                editor.active_field = field;
                terminal.draw(|frame| draw_editor(frame, &editor)).unwrap();
            }
        });
    }

    #[test]
    fn empty_selection_helpers_are_inert() {
        assert_eq!(shifted_index(0, 3, 0), 0);
        assert_eq!(shifted_index(99, -3, 0), 0);
        assert_eq!(scroll_to_visible(0, 0, 0), 0);
        assert_eq!(scroll_to_visible(12, 0, 4), 0);
    }

    #[test]
    fn list_panel_rows_skip_profile_area() {
        let area = Rect::new(0, 0, 30, 20);
        let rows = list_panel_rows_area(area);
        assert_eq!(rows.y, profile_panel_height());
        assert_eq!(
            rows.height,
            area.height.saturating_sub(profile_panel_height())
        );

        let small = Rect::new(0, 0, 30, 8);
        assert_eq!(list_panel_rows_area(small), small);
    }

    #[test]
    fn html_notes_are_rendered_as_plain_text() {
        assert_eq!(
            note_text_for_display("<p>Milk &amp; bread</p><br><strong>Today</strong>"),
            "Milk & bread\nToday"
        );
        assert_eq!(
            note_text_for_editor("<div>Line&nbsp;one</div><br />Line two"),
            "Line one\nLine two"
        );
        assert_eq!(
            note_text_for_display(
                "<style>.x{color:red}</style><p>Plan</p><svg><text>chart label</text></svg><ul><li>One</li><li>Two</li></ul>&#x2713;"
            ),
            "Plan\n- One\n- Two\n✓"
        );
        assert_eq!(
            note_text_for_display("&lt;p&gt;Milk&lt;/p&gt;&lt;p&gt;Bread&lt;/p&gt;"),
            "Milk\nBread"
        );
        assert_eq!(
            note_text_for_display(
                "&lt;style&gt;.x{color:red}&lt;/style&gt;&lt;p&gt;Plan&lt;/p&gt;"
            ),
            "Plan"
        );
    }

    fn test_shopping_list(
        id: i64,
        name: &str,
        icon: Option<&str>,
        folder_id: Option<i64>,
        folder_name: Option<&str>,
        archived: bool,
    ) -> ShoppingList {
        ShoppingList {
            id,
            name: name.to_string(),
            icon: icon.map(str::to_string),
            color: None,
            folder_id,
            folder_name: folder_name.map(str::to_string),
            archived: Some(archived),
            archive_mode: None,
            view_mode: None,
            role: None,
            item_count: None,
            done_count: None,
            state_config: None,
            states: None,
            created_at: None,
        }
    }

    #[test]
    fn foldered_lists_keep_list_icon_slot_and_render_folder_rows() {
        let folder_id_only = test_shopping_list(1, "Groceries", None, Some(83), None, false);
        assert!(list_has_folder(&folder_id_only));
        assert_eq!(list_icon_asset_name(None), Some("tag".to_string()));
        assert_eq!(list_icon_for_tui(None), "[tag]");
        let rows = list_panel_rows(&[folder_id_only]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].list_index, None);
        assert_eq!(rows[0].label, "#83");
        assert_eq!(rows[1].list_index, Some(0));

        let named_folder = test_shopping_list(
            2,
            "Milk",
            Some("bi bi-cart-fill"),
            Some(83),
            Some("Home"),
            false,
        );
        assert!(list_has_folder(&named_folder));
        assert_eq!(
            list_icon_asset_name(named_folder.icon.as_deref()),
            Some("cart-fill".to_string())
        );
    }

    #[test]
    fn list_panel_rows_expand_nested_folder_paths() {
        let lists = vec![
            test_shopping_list(1, "Roadmap", None, Some(10), Some("Work/Backend"), false),
            test_shopping_list(2, "Groceries", None, None, None, false),
        ];
        let rows = list_panel_rows(&lists);

        assert_eq!(rows[0].label, "Work");
        assert_eq!(rows[0].depth, 0);
        assert_eq!(rows[1].label, "Backend");
        assert_eq!(rows[1].depth, 1);
        assert_eq!(rows[2].list_index, Some(0));
        assert_eq!(rows[2].depth, 2);
        assert_eq!(rows[3].list_index, Some(1));
        assert_eq!(selected_list_panel_row(&rows, 1), 3);
    }

    #[test]
    fn list_display_name_for_tui_includes_folder_path() {
        let nested = test_shopping_list(7, "Roadmap", None, Some(11), Some("Work/Backend"), false);
        assert_eq!(list_display_name_for_tui(&nested), "Work/Backend / Roadmap");

        let plain = test_shopping_list(8, "Groceries", None, None, None, false);
        assert_eq!(list_display_name_for_tui(&plain), "Groceries");
    }

    #[test]
    fn item_row_text_keeps_subitem_indent() {
        let mut item = sample_item(42, "Child task");
        item.depth = Some(2);

        assert_eq!(item_row_text(&item), "    [ ] Child task");
    }

    #[test]
    fn item_depths_are_derived_from_parent_ids() {
        let mut items = vec![sample_item(1, "Parent"), sample_item(2, "Child")];
        items[1].parent_item_id = Some(1);

        apply_item_depths(&mut items);

        assert_eq!(items[0].depth, None);
        assert_eq!(items[1].depth, Some(1));
    }

    #[test]
    fn wrap_plain_row_keeps_hanging_indent() {
        assert_eq!(
            wrap_plain_row("> ", "  ", "[ ] abcdefgh", 8),
            vec!["> [ ] ab".to_string(), "  cdefgh".to_string()]
        );
    }

    #[test]
    fn wrapped_item_hit_test_accounts_for_visual_height() {
        let items = vec![sample_item(1, "abcdefghijkl"), sample_item(2, "short")];
        let visible = vec![0, 1];

        assert_eq!(
            visible_item_at_wrapped_row(&items, &visible, 0, 0, 8),
            Some(0)
        );
        assert_eq!(
            visible_item_at_wrapped_row(&items, &visible, 0, 1, 8),
            Some(0)
        );
        assert_eq!(
            visible_item_at_wrapped_row(&items, &visible, 0, 2, 8),
            Some(0)
        );
        assert_eq!(
            visible_item_at_wrapped_row(&items, &visible, 0, 3, 8),
            Some(1)
        );
    }

    #[test]
    fn mouse_buttons_track_left_button_only() {
        assert_eq!(
            MouseButtons::from_kind(MouseEventKind::Down(MouseButton::Left)),
            Some(MouseButtons {
                left_down: true,
                left_drag: false,
                left_up: false,
            })
        );
        assert_eq!(
            MouseButtons::from_kind(MouseEventKind::Drag(MouseButton::Left)),
            Some(MouseButtons {
                left_down: false,
                left_drag: true,
                left_up: false,
            })
        );
        assert_eq!(
            MouseButtons::from_kind(MouseEventKind::Up(MouseButton::Left)),
            Some(MouseButtons {
                left_down: false,
                left_drag: false,
                left_up: true,
            })
        );
        assert_eq!(
            MouseButtons::from_kind(MouseEventKind::Down(MouseButton::Right)),
            None
        );
    }

    #[test]
    fn navigation_action_maps_all_global_keys() {
        assert_eq!(
            navigation_action_for_key(KeyCode::Tab),
            NavigationAction::NextMode
        );
        assert_eq!(
            navigation_action_for_key(KeyCode::BackTab),
            NavigationAction::PreviousMode
        );
        assert_eq!(
            navigation_action_for_key(KeyCode::Char('1')),
            NavigationAction::SwitchMode(ViewMode::List)
        );
        assert_eq!(
            navigation_action_for_key(KeyCode::Char('2')),
            NavigationAction::SwitchMode(ViewMode::Kanban)
        );
        assert_eq!(
            navigation_action_for_key(KeyCode::Char('3')),
            NavigationAction::SwitchMode(ViewMode::Calendar)
        );
        assert_eq!(
            navigation_action_for_key(KeyCode::PageUp),
            NavigationAction::MoveMonth(-1)
        );
        assert_eq!(
            navigation_action_for_key(KeyCode::Char(']')),
            NavigationAction::MoveMonth(1)
        );
        assert_eq!(
            navigation_action_for_key(KeyCode::Left),
            NavigationAction::MoveHorizontal {
                delta: -1,
                fallback: FocusPane::Lists,
            }
        );
        assert_eq!(
            navigation_action_for_key(KeyCode::Right),
            NavigationAction::MoveHorizontal {
                delta: 1,
                fallback: FocusPane::Items,
            }
        );
        assert_eq!(
            navigation_action_for_key(KeyCode::Up),
            NavigationAction::MoveSelection(-1)
        );
        assert_eq!(
            navigation_action_for_key(KeyCode::Down),
            NavigationAction::MoveSelection(1)
        );
        assert_eq!(
            navigation_action_for_key(KeyCode::Enter),
            NavigationAction::Enter
        );
        assert_eq!(
            navigation_action_for_key(KeyCode::Esc),
            NavigationAction::Escape
        );
        assert_eq!(
            navigation_action_for_key(KeyCode::F(1)),
            NavigationAction::Help
        );
        assert_eq!(
            navigation_action_for_key(KeyCode::Home),
            NavigationAction::EdgeItem(true)
        );
        assert_eq!(
            navigation_action_for_key(KeyCode::End),
            NavigationAction::EdgeItem(false)
        );
        assert_eq!(
            navigation_action_for_key(KeyCode::Char('x')),
            NavigationAction::Ignore
        );
    }

    #[test]
    fn view_modes_and_key_binding_sources_cover_branch_variants() {
        assert_eq!(ViewMode::List.next(), ViewMode::Kanban);
        assert_eq!(ViewMode::Kanban.next(), ViewMode::Calendar);
        assert_eq!(ViewMode::Calendar.next(), ViewMode::List);
        assert_eq!(ViewMode::List.prev(), ViewMode::Calendar);
        assert_eq!(ViewMode::Kanban.prev(), ViewMode::List);
        assert_eq!(ViewMode::Calendar.prev(), ViewMode::Kanban);

        let bindings = KeyBindings::from_sources(|action| match action {
            FooterAction::Edit => Some("ctrl+x".to_string()),
            FooterAction::Add => Some("not-a-valid-key".to_string()),
            _ => None,
        });

        assert_eq!(
            bindings.action_for_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL)),
            Some(FooterAction::Edit)
        );
        assert_eq!(
            bindings.action_for_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::empty())),
            Some(FooterAction::Add)
        );
    }

    #[test]
    fn key_binding_parser_covers_alias_function_and_invalid_paths() {
        let shifted_tab = parse_key_binding("shift+tab").expect("shift tab should parse");
        assert_eq!(shifted_tab.code, KeyCode::BackTab);
        assert_eq!(shifted_tab.label, "S+Tab");

        let backtab = parse_key_binding("backtab").expect("backtab should parse");
        assert_eq!(backtab.code, KeyCode::BackTab);
        assert_eq!(backtab.label, "S-Tab");

        let function = parse_key_binding("alt-f12").expect("function key should parse");
        assert_eq!(function.code, KeyCode::F(12));
        assert_eq!(function.label, "A+F12");

        assert!(parse_key_binding("f13").is_none());
        assert!(parse_key_binding("not-a-key").is_none());
        assert!(parse_key_binding("   ").is_none());
    }

    #[test]
    fn layout_and_column_helpers_cover_edge_branches() {
        let state_columns = list_states_to_columns(Some(&[
            ApiListState {
                name: Some(" Open ".to_string()),
                color: None,
                is_done: Some(false),
            },
            ApiListState {
                name: Some("".to_string()),
                color: None,
                is_done: Some(true),
            },
            ApiListState {
                name: Some("Done".to_string()),
                color: None,
                is_done: Some(true),
            },
        ]));
        assert_eq!(state_columns.len(), 2);
        assert_eq!(state_columns[0].name, "Open");
        assert!(state_columns[1].is_done);
        assert!(list_states_to_columns(None).is_empty());

        let (wide_list, wide_detail) = list_mode_layout(Rect::new(0, 0, 120, 24));
        assert_eq!(wide_list.y, wide_detail.y);
        assert!(wide_list.width > wide_detail.width);
        assert!(kanban_chunks(Rect::new(0, 0, 80, 20), 0).is_empty());
        assert_eq!(kanban_visible_range(Rect::new(0, 0, 80, 20), 0, 0), (0, 0));
        assert_eq!(kanban_visible_range(Rect::new(0, 0, 120, 20), 2, 1), (0, 2));
        assert_eq!(kanban_visible_range(Rect::new(0, 0, 58, 20), 5, 4), (3, 2));

        let bindings = KeyBindings {
            bindings: default_key_bindings(),
        };
        assert!(footer_buttons(Rect::new(0, 0, 10, 1), &bindings).is_empty());
        assert!(footer_buttons(Rect::new(0, 0, 1, 5), &bindings).is_empty());
        assert!(!footer_buttons(Rect::new(0, 0, 24, 6), &bindings).is_empty());

        assert_eq!(scroll_to_visible(5, 3, 4), 3);
        assert_eq!(scroll_to_visible(2, 8, 4), 5);
        assert_eq!(scroll_to_visible(2, 3, 4), 2);
    }

    #[test]
    fn calendar_layout_helpers_cover_empty_and_agenda_branches() {
        let mut app = test_app();
        app.lists = vec![test_list()];
        app.items = vec![sample_item(1, "No date")];
        app.calendar_visible_month = Some(SimpleDate {
            year: 2026,
            month: 7,
            day: 1,
        });

        let tiny = app.calendar_layout(Rect::new(0, 0, 1, 1));
        assert!(tiny.month_lines.is_empty());
        assert!(tiny.agenda_lines.is_empty());

        let dated = BTreeMap::new();
        let selected_date = SimpleDate {
            year: 2026,
            month: 7,
            day: 20,
        };
        app.calendar_selected_date = Some(selected_date);
        assert!(app
            .calendar_agenda_entries(&dated, &[], selected_date, 0)
            .is_empty());
        let selected_empty = app.calendar_agenda_entries(&dated, &[], selected_date, 4);
        assert!(selected_empty[0].0.contains("2026-07-20"));

        app.calendar_selected_date = None;
        let month_empty = app.calendar_agenda_entries(&dated, &[], selected_date, 4);
        assert!(!month_empty.is_empty());
    }

    #[tokio::test]
    async fn app_navigation_helpers_update_mode_focus_and_selection() {
        let mut app = test_app();
        app.lists = vec![test_list()];
        app.items = vec![sample_item(1, "One"), sample_item(2, "Two")];
        app.focus = FocusPane::Items;

        let (list_changed, item_changed) = app
            .handle_navigation_key(KeyEvent::new(KeyCode::Down, KeyModifiers::empty()))
            .await
            .unwrap();
        assert!(!list_changed);
        assert!(item_changed);
        assert_eq!(app.selected_item, 1);

        app.handle_navigation_key(KeyEvent::new(KeyCode::Home, KeyModifiers::empty()))
            .await
            .unwrap();
        assert_eq!(app.selected_item, 0);

        app.handle_navigation_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::empty()))
            .await
            .unwrap();
        assert_eq!(app.mode, ViewMode::Kanban);

        app.handle_navigation_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::empty()))
            .await
            .unwrap();
        assert_eq!(app.mode, ViewMode::Calendar);
        app.calendar_selected_date = Some(SimpleDate {
            year: 2026,
            month: 7,
            day: 20,
        });
        app.handle_navigation_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::empty()))
            .await
            .unwrap();
        app.handle_navigation_key(KeyEvent::new(KeyCode::Right, KeyModifiers::empty()))
            .await
            .unwrap();
        assert!(app.calendar_selected_date.is_some());

        app.handle_navigation_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()))
            .await
            .unwrap();
        assert!(app.calendar_selected_date.is_none());

        app.focus = FocusPane::Lists;
        let (list_changed, _) = app
            .handle_navigation_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()))
            .await
            .unwrap();
        assert!(list_changed);
        assert_eq!(app.focus, FocusPane::Items);
    }

    #[tokio::test]
    async fn app_state_helpers_cover_calendar_scroll_and_comment_branches() {
        let mut app = test_app();
        app.lists = vec![test_list()];
        app.items = vec![sample_item(1, "Alpha"), sample_item(2, "Beta")];
        app.items[1].due_date = Some("2026-07-21".to_string());
        app.item_filter = "beta".to_string();
        assert_eq!(app.visible_item_indices(), vec![1]);
        assert_eq!(
            app.default_calendar_date(),
            SimpleDate {
                year: 2026,
                month: 7,
                day: 21
            }
        );

        app.calendar_selected_date = Some(SimpleDate {
            year: 2026,
            month: 7,
            day: 1,
        });
        app.item_filter.clear();
        app.move_calendar_date_selection(20);
        assert_eq!(app.selected_item, 1);
        assert_eq!(
            app.calendar_visible_month,
            Some(SimpleDate {
                year: 2026,
                month: 7,
                day: 1
            })
        );
        app.move_calendar_month_selection(1);
        assert_eq!(
            app.calendar_visible_month,
            Some(SimpleDate {
                year: 2026,
                month: 8,
                day: 1
            })
        );

        let mut no_item_app = test_app();
        no_item_app.load_comments_for_selected_item();
        assert!(no_item_app.comments_cache.is_empty());
        app.comments_cache.insert(1, Vec::new());
        app.selected_item = 0;
        app.load_comments_for_selected_item();
        assert!(app.comments_cache.contains_key(&1));

        app.focus = FocusPane::Lists;
        app.lists
            .push(test_shopping_list(2, "Second", None, None, None, false));
        app.items_cache.insert(2, vec![sample_item(3, "Cached")]);
        app.scroll_active(1);
        assert_eq!(app.selected_list, 1);

        app.focus = FocusPane::Items;
        app.mode = ViewMode::Kanban;
        app.selected_item = 0;
        app.scroll_active(1);
        assert_eq!(app.selected_item, 0);

        app.mode = ViewMode::List;
        app.item_filter = "missing".to_string();
        app.scroll_active(1);
        assert_eq!(app.selected_item, 0);
    }

    #[tokio::test]
    async fn app_apply_helpers_cover_profile_lists_images_and_load_messages() {
        let mut app = test_app();

        app.apply_profile_result(Err("profile failed".to_string()));
        assert_eq!(app.status.as_deref(), Some("profile failed"));

        app.beta_consent_pending = true;
        app.apply_profile_result(Ok(Profile {
            id: Some(1),
            display_name: Some("Ada".to_string()),
            email: Some("ada@example.test".to_string()),
            photo_url: Some(" https://example.test/ada.png ".to_string()),
            lang: Some("en".to_string()),
            is_anonymous: Some(false),
            created_at: None,
            legal: Some(crate::models::ProfileLegalStatus {
                pending: vec![crate::models::ProfilePendingLegalDoc {
                    key: Some("privacy".to_string()),
                }],
            }),
            terms_accepted: Some(false),
        }));
        assert_eq!(app.profile_name.as_deref(), Some("Ada"));
        assert_eq!(
            app.profile_photo_url.as_deref(),
            Some("https://example.test/ada.png")
        );
        assert!(app.legal_consent_pending);

        app.apply_accept_terms_result(Err("accept failed".to_string()));
        assert_eq!(app.status.as_deref(), Some("accept failed"));
        app.apply_accept_terms_result(Ok(
            serde_json::json!({"legal": {"pending": [{"key": "agb"}]}}),
        ));
        assert!(app.legal_consent_pending);
        app.apply_accept_terms_result(Ok(serde_json::json!({"legal": {"pending": []}})));
        assert!(!app.legal_consent_pending);

        app.apply_lists_result(Err("lists failed".to_string()));
        assert_eq!(app.status.as_deref(), Some("lists failed"));
        app.apply_lists_result(Ok(Vec::new()));
        assert_eq!(app.status.as_deref(), Some(tr("output-no-lists").as_str()));
        app.apply_lists_result(Ok(vec![test_list()]));
        assert_eq!(app.selected_list_id(), Some(1));

        app.apply_items_result(2, Ok(vec![sample_item(20, "Cached")]));
        assert!(app.items_cache.contains_key(&2));
        app.apply_items_result(1, Err("items failed".to_string()));
        assert_eq!(app.status.as_deref(), Some("items failed"));
        app.apply_items_result(1, Ok(vec![sample_item(1, "Alpha"), sample_item(2, "Beta")]));
        assert_eq!(app.items.len(), 2);

        app.apply_comments_result(
            1,
            Ok(vec![ItemComment {
                id: 7,
                text: Some("Nice".to_string()),
                user_id: Some(3),
                user_name: Some("Ada".to_string()),
                user_email: None,
                created_at: Some("2026-01-01".to_string()),
            }]),
        );
        assert_eq!(app.comments_cache.get(&1).map(Vec::len), Some(1));

        let mut image_bytes = std::io::Cursor::new(Vec::new());
        DynamicImage::new_rgba8(1, 1)
            .write_to(&mut image_bytes, image::ImageFormat::Png)
            .expect("test image should encode");
        let bytes = image_bytes.into_inner();

        app.pending_detail_image = Some("detail.png".to_string());
        app.apply_detail_image_result("other.png".to_string(), Ok(bytes.clone()));
        assert_eq!(app.pending_detail_image.as_deref(), Some("detail.png"));
        app.apply_detail_image_result("detail.png".to_string(), Err("no image".to_string()));
        assert_eq!(app.detail_image_note.as_deref(), Some("—"));
        app.pending_detail_image = Some("detail.png".to_string());
        app.apply_detail_image_result("detail.png".to_string(), Ok(Vec::new()));
        assert_eq!(app.detail_image_note.as_deref(), Some("—"));
        app.pending_detail_image = Some("detail.png".to_string());
        app.apply_detail_image_result("detail.png".to_string(), Ok(bytes.clone()));
        assert!(app.detail_image.is_some());

        app.pending_profile_image = Some("profile.png".to_string());
        app.apply_profile_image_result("other.png".to_string(), Ok(bytes.clone()));
        assert_eq!(app.pending_profile_image.as_deref(), Some("profile.png"));
        app.apply_profile_image_result("profile.png".to_string(), Err("no image".to_string()));
        assert!(app.profile_image.is_none());
        app.pending_profile_image = Some("profile.png".to_string());
        app.apply_profile_image_result("profile.png".to_string(), Ok(bytes));
        assert!(app.profile_image.is_some());

        app.pending_profile_image = Some("profile.png".to_string());
        app.apply_profile_image_result("profile.png".to_string(), Ok(vec![1, 2, 3]));
        assert!(app.profile_image.is_none());

        app.set_inline_images_enabled(true);
        app.pending_profile_image = Some("stale.png".to_string());
        app.profile_photo_url = None;
        app.refresh_profile_image_background();
        assert!(app.pending_profile_image.is_none());

        app.pending_list_icons.insert("cart".to_string());
        app.apply_list_icon_result("cart".to_string(), Err("missing".to_string()));
        assert!(app.failed_list_icons.contains("cart"));
        app.pending_list_icons.insert("tag".to_string());
        app.apply_list_icon_result("tag".to_string(), Ok(DynamicImage::new_rgba8(2, 2)));
        assert!(!app.pending_list_icons.contains("tag"));

        app.pending_open_image = Some("expected.png".to_string());
        app.status = Some("unchanged".to_string());
        app.apply_open_image_result("other.png".to_string(), Ok("ignored".to_string()));
        assert_eq!(app.pending_open_image.as_deref(), Some("expected.png"));
        assert_eq!(app.status.as_deref(), Some("unchanged"));

        app.pending_open_image = Some("bad.png".to_string());
        app.apply_open_image_result("bad.png".to_string(), Err("open failed".to_string()));
        assert_eq!(app.status.as_deref(), Some("open failed"));
        app.pending_open_image = Some("ok.png".to_string());
        app.apply_open_image_result("ok.png".to_string(), Ok("opened".to_string()));
        assert!(app
            .status
            .as_deref()
            .is_some_and(|value| value.contains("opened")));

        app.tx
            .send(LoadMessage::Comments {
                item_id: 2,
                result: Ok(Vec::new()),
            })
            .expect("load message should send");
        app.tx
            .send(LoadMessage::AutoHandoffSent)
            .expect("load message should send");
        assert!(app.drain_load_messages());
        assert!(app.comments_cache.contains_key(&2));
        assert!(!app.drain_load_messages());
    }

    #[tokio::test]
    async fn background_load_helpers_cover_reload_and_auto_handoff_paths() {
        crate::test_env::with_env_lock_async(|| async {
            let previous_auto_handoff = std::env::var_os(KRAMLI_AUTO_HANDOFF_ENV);

            std::env::set_var(KRAMLI_AUTO_HANDOFF_ENV, "0");
            let (api, requests) = api_with_responses(vec![
                serde_json::json!([{"id": 1, "name": "Groceries"}]).to_string(),
                serde_json::json!([{"id": 11, "list_id": 1, "text": "Milk", "is_done": false}])
                    .to_string(),
            ])
            .await;
            let mut app = App::new(api, true);
            app.reload_lists_background();

            for _ in 0..80 {
                let _ = app.drain_load_messages();
                if !app.loading_lists && app.loading_items_for.is_none() && app.items.len() == 1 {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }

            assert_eq!(app.lists.len(), 1);
            assert_eq!(app.items.len(), 1);
            let list_requests = requests.await.expect("test server should finish");
            assert_eq!(
                list_requests,
                vec![
                    "GET /api/lists HTTP/1.1",
                    "GET /api/lists/1/items HTTP/1.1"
                ]
            );

            std::env::set_var(KRAMLI_AUTO_HANDOFF_ENV, "1");
            let (api, requests) =
                api_with_responses(vec![serde_json::json!({"ok": true}).to_string()]).await;
            let mut app = App::new(api, true);

            app.send_auto_handoff_viewing_background();
            assert_eq!(app.pending_auto_handoff_list_id, None);

            app.lists = vec![test_list()];
            app.selected_list = 0;
            app.send_auto_handoff_viewing_background();
            assert_eq!(app.pending_auto_handoff_list_id, Some(1));

            let mut due_processed = false;
            for _ in 0..120 {
                if app.drain_load_messages() {
                    due_processed = true;
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            assert!(due_processed);

            let handoff_requests = requests.await.expect("test server should finish");
            assert_eq!(handoff_requests, vec!["POST /api/activity/viewing HTTP/1.1"]);

            match previous_auto_handoff {
                Some(value) => std::env::set_var(KRAMLI_AUTO_HANDOFF_ENV, value),
                None => std::env::remove_var(KRAMLI_AUTO_HANDOFF_ENV),
            }
        })
        .await;
    }

    #[tokio::test]
    async fn footer_actions_cover_item_member_invite_and_undo_paths() {
        crate::test_env::with_env_lock_async(|| async {
            let previous_auto_handoff = std::env::var_os(KRAMLI_AUTO_HANDOFF_ENV);
            std::env::set_var(KRAMLI_AUTO_HANDOFF_ENV, "0");

            let (api, requests) = api_with_responses(vec![
                serde_json::json!({"id": 1, "list_id": 1, "text": "Milk", "is_done": true})
                    .to_string(),
                serde_json::json!({"ok": true}).to_string(),
                serde_json::json!([
                    {"display_name": "Ada"},
                    {"display_name": "Grace"},
                    {"display_name": "Linus"},
                    {"display_name": "Ken"}
                ])
                .to_string(),
                serde_json::json!({"invite_url": "https://kram.li/i/from-test"}).to_string(),
                serde_json::json!({}).to_string(),
                serde_json::json!({"ok": true}).to_string(),
                serde_json::json!([{"id": 1, "list_id": 1, "text": "Reloaded", "is_done": false}])
                    .to_string(),
                serde_json::json!([]).to_string(),
            ])
            .await;
            let mut app = App::new(api, true);
            app.lists = vec![test_list()];
            app.items = vec![sample_item(1, "Milk")];
            app.items_cache.insert(1, app.items.clone());

            app.trigger_footer_action(FooterAction::ToggleDone)
                .await
                .unwrap();
            assert_eq!(app.items[0].is_done, Some(true));

            app.trigger_footer_action(FooterAction::Delete).await.unwrap();
            assert!(app.items.is_empty());

            app.trigger_footer_action(FooterAction::Members).await.unwrap();
            assert!(app
                .status
                .as_deref()
                .is_some_and(|value| value.contains("4")));

            app.trigger_footer_action(FooterAction::Invite).await.unwrap();
            assert!(app
                .status
                .as_deref()
                .is_some_and(|value| value.contains("https://kram.li/i/from-test")));

            app.trigger_footer_action(FooterAction::Invite).await.unwrap();
            assert!(app
                .status
                .as_deref()
                .is_some_and(|value| value.contains("-")));

            app.comments_cache.insert(1, Vec::new());
            app.trigger_footer_action(FooterAction::Undo).await.unwrap();
            for _ in 0..120 {
                let _ = app.drain_load_messages();
                if app.loading_items_for.is_none() && !app.items.is_empty() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            assert_eq!(app.items.len(), 1);

            app.trigger_footer_action(FooterAction::Members).await.unwrap();
            assert_eq!(app.status, Some(tr("output-no-members")));

            app.trigger_footer_action(FooterAction::Help).await.unwrap();
            assert!(app.show_help);
            app.trigger_footer_action(FooterAction::Quit).await.unwrap();
            assert!(app.should_quit);

            app.lists.clear();
            app.items.clear();
            app.trigger_footer_action(FooterAction::ToggleDone)
                .await
                .unwrap();
            app.trigger_footer_action(FooterAction::Delete).await.unwrap();
            app.trigger_footer_action(FooterAction::Members).await.unwrap();
            app.trigger_footer_action(FooterAction::Invite).await.unwrap();
            app.trigger_footer_action(FooterAction::Undo).await.unwrap();

            let requests = requests.await.expect("test server should finish");
            assert_eq!(
                requests,
                vec![
                    "PATCH /api/items/1/done HTTP/1.1",
                    "DELETE /api/items/1 HTTP/1.1",
                    "GET /api/lists/1/members HTTP/1.1",
                    "POST /api/lists/1/invite-link HTTP/1.1",
                    "POST /api/lists/1/invite-link HTTP/1.1",
                    "POST /api/lists/1/undo HTTP/1.1",
                    "GET /api/lists/1/items HTTP/1.1",
                    "GET /api/lists/1/members HTTP/1.1"
                ]
            );

            match previous_auto_handoff {
                Some(value) => std::env::set_var(KRAMLI_AUTO_HANDOFF_ENV, value),
                None => std::env::remove_var(KRAMLI_AUTO_HANDOFF_ENV),
            }
        })
        .await;
    }

    #[tokio::test]
    async fn navigation_and_key_helpers_cover_remaining_mode_paths() {
        let mut app = test_app();
        app.lists = vec![
            test_list(),
            test_shopping_list(2, "Second", None, None, None, false),
        ];
        app.items = vec![sample_item(1, "Alpha"), sample_item(2, "Beta")];
        app.items[0].due_date = Some("2026-07-01".to_string());
        app.selected_item = 0;

        app.switch_mode(ViewMode::Calendar);
        assert_eq!(app.mode, ViewMode::Calendar);
        assert!(app.calendar_selected_date.is_some());

        let month_before = app.calendar_visible_month;
        app.focus = FocusPane::Lists;
        app.move_month_if_calendar_items(1);
        assert_eq!(app.calendar_visible_month, month_before);

        app.focus = FocusPane::Items;
        app.move_month_if_calendar_items(1);
        assert_ne!(app.calendar_visible_month, month_before);

        app.focus = FocusPane::Lists;
        app.move_horizontal_or_focus(1, FocusPane::Items);
        assert_eq!(app.focus, FocusPane::Items);

        let date_before = app.calendar_selected_date;
        app.move_horizontal_or_focus(1, FocusPane::Lists);
        assert_ne!(app.calendar_selected_date, date_before);

        let mut list_changed = false;
        let mut item_changed = false;
        app.focus = FocusPane::Lists;
        app.move_selection_by_key(
            KeyEvent::new(KeyCode::Down, KeyModifiers::empty()),
            1,
            &mut list_changed,
            &mut item_changed,
        )
        .await
        .unwrap();
        assert!(list_changed);

        app.mode = ViewMode::List;
        app.focus = FocusPane::Items;
        assert!(app.move_visible_list_selection(1));

        app.mode = ViewMode::Calendar;
        app.calendar_selected_date = Some(SimpleDate {
            year: 2026,
            month: 7,
            day: 2,
        });
        assert!(!app
            .move_selected_item_by_key(KeyEvent::new(KeyCode::Down, KeyModifiers::empty()), 1)
            .await
            .unwrap());

        app.mode = ViewMode::Kanban;
        let _ = app
            .move_selected_item_by_key(KeyEvent::new(KeyCode::Down, KeyModifiers::empty()), 1)
            .await
            .unwrap();

        app.mode = ViewMode::List;
        app.focus = FocusPane::Lists;
        assert!(app
            .handle_enter_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()))
            .unwrap());

        app.mode = ViewMode::Calendar;
        app.focus = FocusPane::Items;
        app.calendar_selected_date = Some(SimpleDate {
            year: 2026,
            month: 7,
            day: 3,
        });
        assert!(!app
            .handle_enter_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL))
            .unwrap());
        assert!(app.editor.is_some());
        app.editor = None;

        assert!(!app
            .handle_enter_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()))
            .unwrap());
        assert!(app.editor.is_some());
        app.editor = None;

        app.calendar_drag_item = Some(0);
        app.handle_escape_key();
        assert!(app.calendar_drag_item.is_none());
        app.calendar_selected_date = Some(SimpleDate {
            year: 2026,
            month: 7,
            day: 3,
        });
        app.handle_escape_key();
        assert!(app.calendar_selected_date.is_none());

        app.item_filter = "missing".to_string();
        assert!(!app.select_visible_edge_item(true));
        app.item_filter.clear();
        app.selected_item = 1;
        assert!(app.select_visible_edge_item(true));
        assert_eq!(app.selected_item, 0);
        assert!(app.select_visible_edge_item(false));
        assert_eq!(app.selected_item, 1);

        app.lists.clear();
        app.items.clear();
        app.apply_key_change_effects(true, false);
        assert!(app.calendar_selected_date.is_none());

        app.lists = vec![test_list()];
        app.items = vec![sample_item(5, "Five")];
        app.selected_item = 0;
        app.comments_cache.insert(5, Vec::new());
        app.mode = ViewMode::List;
        app.apply_key_change_effects(false, true);

        app.mode = ViewMode::Calendar;
        app.apply_key_change_effects(false, true);
        assert!(app.calendar_selected_date.is_some());

        app.show_help = true;
        app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::empty()))
            .await
            .unwrap();
        assert!(!app.show_help);

        app.mode = ViewMode::List;
        app.focus = FocusPane::Items;
        app.comments_cache.insert(5, Vec::new());
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::empty()))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn editor_key_and_save_helpers_cover_refactored_branches() {
        let mut app = test_app();
        app.lists = vec![test_list()];
        app.items = vec![sample_item(1, "Alpha"), sample_item(2, "Beta")];

        app.open_add_editor().unwrap();
        assert_eq!(app.editor.as_ref().unwrap().active_field, EditorField::Text);
        app.handle_editor_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::empty()))
            .await
            .unwrap();
        assert_eq!(
            app.editor.as_ref().unwrap().active_field,
            EditorField::Quantity
        );
        app.handle_editor_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::empty()))
            .await
            .unwrap();
        assert_eq!(app.editor.as_ref().unwrap().active_field, EditorField::Text);
        app.handle_editor_key(KeyEvent::new(KeyCode::Down, KeyModifiers::empty()))
            .await
            .unwrap();
        assert_eq!(
            app.editor.as_ref().unwrap().active_field,
            EditorField::Quantity
        );
        app.handle_editor_key(KeyEvent::new(KeyCode::Left, KeyModifiers::empty()))
            .await
            .unwrap();
        assert_eq!(app.editor.as_ref().unwrap().active_field, EditorField::Text);
        app.handle_editor_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::empty()))
            .await
            .unwrap();
        assert_eq!(app.editor.as_ref().unwrap().text, "x");
        app.handle_editor_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::empty()))
            .await
            .unwrap();
        assert_eq!(app.editor.as_ref().unwrap().text, "");
        app.handle_editor_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::empty()))
            .await
            .unwrap();
        app.handle_editor_key(KeyEvent::new(KeyCode::Delete, KeyModifiers::empty()))
            .await
            .unwrap();
        assert_eq!(app.editor.as_ref().unwrap().text, "");
        app.handle_editor_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()))
            .await
            .unwrap();
        assert_eq!(
            app.editor.as_ref().unwrap().active_field,
            EditorField::Quantity
        );

        app.open_filter_editor().unwrap();
        app.handle_editor_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::empty()))
            .await
            .unwrap();
        assert_eq!(app.editor.as_ref().unwrap().active_field, EditorField::Text);
        app.editor.as_mut().unwrap().text = "beta".to_string();
        let editor = app.editor.as_ref().unwrap().clone();
        app.save_filter_editor(&editor).unwrap();
        assert_eq!(app.item_filter, "beta");
        assert_eq!(app.selected_item, 1);
        assert!(app.status.as_deref().unwrap_or_default().contains("beta"));

        app.open_filter_editor().unwrap();
        app.editor.as_mut().unwrap().text.clear();
        let editor = app.editor.as_ref().unwrap().clone();
        app.save_filter_editor(&editor).unwrap();
        assert!(app.item_filter.is_empty());
        assert_eq!(app.status, Some(tr("label-items")));

        let editor = EditorState {
            mode: EditorMode::Comment,
            item_id: None,
            text: String::default(),
            quantity: String::default(),
            due_date: String::default(),
            due_time: String::default(),
            planned_date: String::default(),
            planned_time: String::default(),
            reminder: String::default(),
            reminder_time: String::default(),
            reminder_offsets: String::default(),
            travel_time_minutes: String::default(),
            priority: String::default(),
            tags: String::default(),
            progress: String::default(),
            notes: String::default(),
            active_field: EditorField::Text,
        };
        app.save_comment_editor(&editor, "Hello").await.unwrap();
        assert!(app.comments_cache.is_empty());
    }

    #[tokio::test]
    async fn comment_editor_save_posts_and_updates_cache() {
        let (api, requests) = api_with_responses(vec![
            serde_json::json!({"id": 9, "text": "Hello"}).to_string(),
        ])
        .await;
        let mut app = App::new(api, true);
        let editor = EditorState {
            mode: EditorMode::Comment,
            item_id: Some(7),
            text: String::default(),
            quantity: String::default(),
            due_date: String::default(),
            due_time: String::default(),
            planned_date: String::default(),
            planned_time: String::default(),
            reminder: String::default(),
            reminder_time: String::default(),
            reminder_offsets: String::default(),
            travel_time_minutes: String::default(),
            priority: String::default(),
            tags: String::default(),
            progress: String::default(),
            notes: String::default(),
            active_field: EditorField::Text,
        };

        app.save_comment_editor(&editor, "Hello").await.unwrap();

        assert_eq!(app.comments_cache.get(&7).map(Vec::len), Some(1));
        assert!(app.editor.is_none());
        assert_eq!(app.status, Some(tr("label-comments")));
        let requests = requests.await.expect("test server should finish");
        assert_eq!(requests, vec!["POST /api/items/7/comments HTTP/1.1"]);
    }

    #[tokio::test]
    async fn save_editor_delegates_filter_and_comment_modes() {
        let (api, requests) = api_with_responses(vec![
            serde_json::json!({"id": 11, "text": "Posted"}).to_string(),
        ])
        .await;
        let mut app = App::new(api, true);
        app.lists = vec![test_list()];
        app.items = vec![sample_item(7, "Alpha"), sample_item(8, "Beta")];

        app.editor = Some(EditorState {
            mode: EditorMode::Filter,
            item_id: None,
            text: "beta".to_string(),
            quantity: String::default(),
            due_date: String::default(),
            due_time: String::default(),
            planned_date: String::default(),
            planned_time: String::default(),
            reminder: String::default(),
            reminder_time: String::default(),
            reminder_offsets: String::default(),
            travel_time_minutes: String::default(),
            priority: String::default(),
            tags: String::default(),
            progress: String::default(),
            notes: String::default(),
            active_field: EditorField::Text,
        });
        app.save_editor().await.unwrap();
        assert_eq!(app.item_filter, "beta");
        assert_eq!(app.selected_item, 1);

        app.editor = Some(EditorState {
            mode: EditorMode::Comment,
            item_id: Some(7),
            text: "Posted".to_string(),
            quantity: String::default(),
            due_date: String::default(),
            due_time: String::default(),
            planned_date: String::default(),
            planned_time: String::default(),
            reminder: String::default(),
            reminder_time: String::default(),
            reminder_offsets: String::default(),
            travel_time_minutes: String::default(),
            priority: String::default(),
            tags: String::default(),
            progress: String::default(),
            notes: String::default(),
            active_field: EditorField::Text,
        });
        app.save_editor().await.unwrap();
        assert_eq!(app.comments_cache.get(&7).map(Vec::len), Some(1));
        let requests = requests.await.expect("test server should finish");
        assert_eq!(requests, vec!["POST /api/items/7/comments HTTP/1.1"]);
    }

    #[tokio::test]
    async fn editor_key_helpers_cover_remaining_motion_paths() {
        let mut app = test_app();
        app.lists = vec![test_list()];
        app.items = vec![sample_item(1, "Alpha")];

        app.open_filter_editor().unwrap();
        app.handle_editor_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::empty()))
            .await
            .unwrap();
        app.handle_editor_key(KeyEvent::new(KeyCode::Up, KeyModifiers::empty()))
            .await
            .unwrap();
        assert_eq!(app.editor.as_ref().unwrap().active_field, EditorField::Text);

        app.open_add_editor().unwrap();
        app.editor.as_mut().unwrap().active_field = EditorField::Text;
        app.handle_editor_key(KeyEvent::new(KeyCode::Right, KeyModifiers::empty()))
            .await
            .unwrap();
        assert_eq!(
            app.editor.as_ref().unwrap().active_field,
            EditorField::Quantity
        );

        app.editor.as_mut().unwrap().active_field = EditorField::DueDate;
        app.handle_editor_key(KeyEvent::new(KeyCode::Up, KeyModifiers::empty()))
            .await
            .unwrap();
        assert_eq!(
            app.editor.as_ref().unwrap().active_field,
            EditorField::Quantity
        );

        app.editor.as_mut().unwrap().active_field = EditorField::Progress;
        app.editor.as_mut().unwrap().progress = "O".to_string();
        app.handle_editor_key(KeyEvent::new(KeyCode::Down, KeyModifiers::empty()))
            .await
            .unwrap();
        assert_eq!(
            app.editor.as_ref().unwrap().active_field,
            EditorField::Progress
        );

        app.open_filter_editor().unwrap();
        app.editor.as_mut().unwrap().text = "Alpha".to_string();
        app.handle_editor_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()))
            .await
            .unwrap();
        assert!(app.editor.is_none());
    }

    #[test]
    fn empty_state_and_progress_helpers_cover_remaining_branches() {
        assert_eq!(editor_bool_label(Some(true)), tr("label-on"));
        assert_eq!(editor_bool_label(Some(false)), tr("label-off"));

        let mut app = test_app();
        app.items = vec![sample_item(1, "Orphan")];
        app.selected_item = 0;
        app.load_items_for_selected_list(true);
        assert!(app.items.is_empty());
        assert_eq!(app.selected_item, 0);
        assert!(app.detail_image.is_none());
        assert!(app.detail_image_note.is_none());

        app.focus = FocusPane::Lists;
        app.scroll_active(1);
        assert_eq!(app.selected_list, 0);
        assert!(!app.move_selected_list(1));

        app.lists = vec![
            test_list(),
            test_shopping_list(2, "Second", None, None, None, false),
        ];
        assert!(app.move_selected_list(1));
        assert_eq!(app.selected_list, 1);

        let mut done_only = test_list();
        done_only.states = Some(vec![ApiListState {
            name: Some("Done".to_string()),
            color: None,
            is_done: Some(true),
        }]);
        app.lists = vec![done_only];
        app.selected_list = 0;
        assert_eq!(app.default_progress_value(), "Done");
        assert!(!app.move_kanban_selection(0));
    }

    #[test]
    fn editor_opening_helpers_handle_empty_and_default_paths() {
        let mut app = test_app();

        app.open_editor().unwrap();
        assert_eq!(app.status, Some(tr("output-no-items")));

        app.status = None;
        app.open_add_editor().unwrap();
        assert_eq!(app.status, Some(tr("output-no-lists")));

        app.lists = vec![test_list()];
        app.items = vec![sample_item(1, "One")];
        app.focus = FocusPane::Items;
        app.mode = ViewMode::List;
        assert!(!app
            .handle_enter_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()))
            .unwrap());
        assert!(app.editor.is_some());
    }

    #[tokio::test]
    async fn app_navigation_and_mouse_helpers_cover_false_and_editor_branches() {
        let mut app = test_app();
        app.lists = vec![test_list()];
        app.items = vec![sample_item(1, "One")];
        app.items_cache.insert(1, app.items.clone());

        app.handle_navigation_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::empty()))
            .await
            .unwrap();
        assert_eq!(app.mode, ViewMode::Calendar);
        app.handle_navigation_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::empty()))
            .await
            .unwrap();
        assert!(app.show_help);
        app.show_help = false;

        app.mode = ViewMode::List;
        app.focus = FocusPane::Items;
        app.handle_navigation_key(KeyEvent::new(KeyCode::Left, KeyModifiers::empty()))
            .await
            .unwrap();
        assert_eq!(app.focus, FocusPane::Lists);

        let (changed, _) = app
            .handle_navigation_key(KeyEvent::new(KeyCode::Down, KeyModifiers::empty()))
            .await
            .unwrap();
        assert!(!changed);

        app.mode = ViewMode::Calendar;
        app.focus = FocusPane::Items;
        app.calendar_selected_date = Some(SimpleDate {
            year: 2026,
            month: 7,
            day: 20,
        });
        app.handle_navigation_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()))
            .await
            .unwrap();
        assert!(app.editor.is_some());
        app.editor = None;

        app.handle_navigation_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL))
            .await
            .unwrap();
        assert!(app.editor.is_some());
        app.editor = None;

        app.calendar_drag_item = Some(0);
        app.handle_navigation_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()))
            .await
            .unwrap();
        assert!(app.calendar_drag_item.is_none());

        app.item_filter = "missing".to_string();
        let (_, item_changed) = app
            .handle_navigation_key(KeyEvent::new(KeyCode::Home, KeyModifiers::empty()))
            .await
            .unwrap();
        assert!(!item_changed);

        app.item_filter.clear();
        assert!(!app.handle_help_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 1, 1)));
        assert!(!app.handle_mouse_scroll(mouse(MouseEventKind::Down(MouseButton::Left), 1, 1)));

        let area = Rect::new(0, 0, 120, 40);
        let layout = ui_layout(area);
        assert!(!app.handle_tab_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                layout.footer.x,
                layout.footer.y
            ),
            &layout,
            true,
        ));
        assert!(!app.handle_tab_mouse(
            mouse(
                MouseEventKind::Up(MouseButton::Left),
                layout.tab_chunks[0].x,
                layout.tab_chunks[0].y
            ),
            &layout,
            false,
        ));
        assert!(!app
            .handle_footer_mouse(
                mouse(MouseEventKind::Up(MouseButton::Left), 0, 0),
                layout.footer,
                false
            )
            .await
            .unwrap());
        assert!(!app.handle_list_panel_mouse(
            mouse(
                MouseEventKind::Up(MouseButton::Left),
                layout.lists.x,
                layout.lists.y
            ),
            layout.lists,
            false,
        ));
        assert!(!app.handle_non_content_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                layout.content.x,
                layout.content.y
            ),
            &layout,
            false,
        ));
        assert!(!app.handle_empty_items_mouse(false));

        app.trigger_footer_action(FooterAction::Add).await.unwrap();
        assert!(app.editor.is_some());
    }

    #[tokio::test]
    async fn app_mouse_helpers_cover_tabs_footer_lists_and_modes() {
        let mut app = test_app();
        app.lists = vec![test_list()];
        app.items = vec![sample_item(1, "One"), sample_item(2, "Two")];
        app.items_cache.insert(1, app.items.clone());
        let area = Rect::new(0, 0, 120, 40);
        let layout = ui_layout(area);

        app.show_help = true;
        assert!(app.handle_help_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 1, 1)));
        assert!(!app.show_help);

        app.focus = FocusPane::Items;
        app.mode = ViewMode::List;
        assert!(app.handle_mouse_scroll(MouseEvent {
            modifiers: KeyModifiers::empty(),
            ..mouse(MouseEventKind::ScrollDown, 1, 1)
        }));

        let tab = layout.tab_chunks[1];
        assert!(app.handle_tab_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                tab.x + 1,
                tab.y + 1
            ),
            &layout,
            true,
        ));
        assert_eq!(app.mode, ViewMode::Kanban);

        let help_button = footer_buttons(layout.footer, &app.key_bindings)
            .into_iter()
            .find(|(action, _)| *action == FooterAction::Help)
            .unwrap()
            .1;
        assert!(app
            .handle_footer_mouse(
                mouse(
                    MouseEventKind::Down(MouseButton::Left),
                    help_button.x,
                    help_button.y
                ),
                layout.footer,
                true,
            )
            .await
            .unwrap());
        assert!(app.show_help);
        app.show_help = false;

        let list_rows = item_rows_area(list_panel_rows_area(layout.lists));
        assert!(app.handle_list_panel_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                list_rows.x,
                list_rows.y + 1,
            ),
            layout.lists,
            true,
        ));
        assert_eq!(app.selected_list, 0);

        assert!(app.handle_non_content_mouse(
            mouse(
                MouseEventKind::Up(MouseButton::Left),
                layout.footer.x,
                layout.footer.y
            ),
            &layout,
            true,
        ));

        let mut empty_app = test_app();
        assert!(empty_app.handle_empty_items_mouse(true));
        assert!(empty_app.status.is_some());

        app.mode = ViewMode::List;
        let (list_rect, _) = list_mode_layout(layout.content);
        let list_items_area = item_rows_area(list_rect);
        app.handle_mode_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                list_items_area.x,
                list_items_area.y,
            ),
            layout.content,
            MouseButtons {
                left_down: true,
                left_drag: false,
                left_up: false,
            },
        )
        .await
        .unwrap();

        app.mode = ViewMode::Kanban;
        app.items[0].progress = Some(tr("tui-kanban-open"));
        let (columns, buckets) = app.kanban_buckets();
        let chunks = kanban_chunks(layout.content, columns.len().min(3));
        let column_area = item_rows_area(chunks[0]);
        assert!(!buckets[0].is_empty());
        app.handle_kanban_mode_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                column_area.x,
                column_area.y,
            ),
            layout.content,
            true,
            false,
            false,
        )
        .await
        .unwrap();
        assert!(app.kanban_drag_item.is_some());
        app.handle_kanban_mode_mouse(
            mouse(
                MouseEventKind::Drag(MouseButton::Left),
                column_area.x,
                column_area.y,
            ),
            layout.content,
            false,
            true,
            false,
        )
        .await
        .unwrap();
        app.handle_kanban_mode_mouse(
            mouse(
                MouseEventKind::Up(MouseButton::Left),
                column_area.x,
                column_area.y,
            ),
            layout.content,
            false,
            false,
            true,
        )
        .await
        .unwrap();

        app.mode = ViewMode::Calendar;
        app.items[0].due_date = Some("2026-07-20".to_string());
        app.calendar_selected_date = Some(SimpleDate {
            year: 2026,
            month: 7,
            day: 20,
        });
        app.calendar_visible_month = Some(SimpleDate {
            year: 2026,
            month: 7,
            day: 1,
        });
        let calendar = app.calendar_layout(layout.content);
        let item_hit = calendar.item_hits[0].rect;
        app.handle_calendar_mode_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                item_hit.x,
                item_hit.y,
            ),
            layout.content,
            true,
            false,
            false,
        )
        .await
        .unwrap();
        app.handle_calendar_mode_mouse(
            mouse(
                MouseEventKind::Drag(MouseButton::Left),
                item_hit.x,
                item_hit.y,
            ),
            layout.content,
            false,
            true,
            false,
        )
        .await
        .unwrap();
        app.handle_calendar_mode_mouse(
            mouse(
                MouseEventKind::Up(MouseButton::Left),
                item_hit.x,
                item_hit.y,
            ),
            layout.content,
            false,
            false,
            true,
        )
        .await
        .unwrap();
    }

    #[test]
    fn sort_lists_for_tui_groups_by_folder_then_name() {
        let mut lists = vec![
            test_shopping_list(1, "Zed", None, None, None, false),
            test_shopping_list(2, "Bananas", None, Some(20), Some("Work"), false),
            test_shopping_list(3, "Apples", None, Some(10), Some("Home"), false),
            test_shopping_list(4, "Avocado", None, Some(20), Some("Work"), false),
            test_shopping_list(5, "Alpha", None, None, None, false),
        ];

        sort_lists_for_tui(&mut lists);
        let ordered_names = lists
            .iter()
            .map(|list| list.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            ordered_names,
            vec!["Apples", "Avocado", "Bananas", "Alpha", "Zed"]
        );
    }

    #[test]
    fn bootstrap_icons_use_asset_names_and_text_fallbacks_without_emoji() {
        assert_eq!(
            bootstrap_icon_asset_name(Some("bi bi-cart-fill")),
            Some("cart-fill".to_string())
        );
        assert_eq!(
            bootstrap_icon_asset_name(Some("bi-0-circle")),
            Some("0-circle".to_string())
        );
        assert_eq!(
            bootstrap_icon_asset_name(Some(r#"<i class="bi bi-basket-fill"></i>"#)),
            Some("basket-fill".to_string())
        );
        assert_eq!(
            bootstrap_icon_asset_name(Some(
                "https://icons.getbootstrap.com/assets/icons/0-circle.svg"
            )),
            Some("0-circle".to_string())
        );
        assert_eq!(
            bootstrap_icon_asset_name(Some("bootstrap-icons:calendar_check")),
            Some("calendar-check".to_string())
        );
        assert_eq!(
            bootstrap_icon_asset_name(Some("basket")),
            Some("basket".to_string())
        );
        assert_eq!(bootstrap_icon_asset_name(Some("bi-../../secret")), None);
        assert_eq!(
            bootstrap_icon_for_tui("bi-book-fill", TuiIconStyle::Label),
            "[book-fill]"
        );
        assert_eq!(
            bootstrap_icon_for_tui("bi-unknown-icon", TuiIconStyle::Raw),
            "bi-unknown-icon"
        );
        assert_eq!(list_icon_asset_name(None), Some("tag".to_string()));
        assert_eq!(
            list_icon_asset_name(Some(r#"<i class="bi bi-basket-fill"></i>"#)),
            Some("basket-fill".to_string())
        );
        assert_eq!(
            list_icon_asset_name(Some("bi bi-cart-fill")),
            Some("cart-fill".to_string())
        );
        assert_eq!(normalize_list_icon(Some("bi bi-cart-fill")), "[cart-fill]");
        assert_eq!(normalize_list_icon(None), "[tag]");
        assert_eq!(normalize_list_icon(Some("not an icon?")), "[tag]");
        assert_eq!(list_icon_for_tui(Some("bi bi-cart-fill")), "[cart-fill]");
        assert_eq!(
            bootstrap_icon_for_tui(&format!("bi-{ARCHIVED_LIST_ICON}"), TuiIconStyle::Label),
            "[archive]"
        );
        assert!(!list_icon_images_supported(ProtocolType::Halfblocks));
        assert!(list_icon_images_supported(ProtocolType::Kitty));
        assert!(list_icon_image_enabled(
            true,
            true,
            ProtocolType::Kitty,
            Some("cart-fill")
        ));
        assert!(!list_icon_image_enabled(
            false,
            true,
            ProtocolType::Kitty,
            Some("cart-fill")
        ));
        assert!(!list_icon_image_enabled(
            true,
            false,
            ProtocolType::Kitty,
            Some("cart-fill")
        ));
        assert!(!list_icon_image_enabled(
            true,
            true,
            ProtocolType::Halfblocks,
            Some("cart-fill")
        ));
        assert!(!list_icon_image_enabled(
            true,
            true,
            ProtocolType::Kitty,
            None
        ));
    }

    #[test]
    fn tui_icon_style_reads_raw_environment_value() {
        crate::test_env::with_env_lock(|| {
            std::env::set_var(KRAMLI_ICON_STYLE_ENV, "raw");
            assert_eq!(tui_icon_style(), TuiIconStyle::Raw);
            std::env::set_var(KRAMLI_ICON_STYLE_ENV, "unexpected");
            assert_eq!(tui_icon_style(), TuiIconStyle::Label);
            std::env::remove_var(KRAMLI_ICON_STYLE_ENV);
            assert_eq!(tui_icon_style(), TuiIconStyle::Label);
        });
    }

    #[test]
    fn bootstrap_sprite_contains_icons_from_official_symbol_ids() {
        let sprite = r#"
            <svg xmlns="http://www.w3.org/2000/svg">
              <symbol id="basket-fill" viewBox="0 0 16 16"></symbol>
              <symbol id="0-circle" viewBox="0 0 16 16"></symbol>
            </svg>
        "#;
        let parsed = bootstrap_icon_asset_name(Some(r#"<i class="bi bi-basket-fill"></i>"#))
            .expect("class icon should parse");
        assert!(bootstrap_sprite_contains_icon(sprite, &parsed));
        assert!(bootstrap_sprite_contains_icon(sprite, "0-circle"));
        assert!(!bootstrap_sprite_contains_icon(sprite, "missing-icon"));
        assert!(!bootstrap_sprite_contains_icon(sprite, "../secret"));
    }

    #[test]
    fn bootstrap_icon_color_follows_theme_and_env_hints() {
        assert_eq!(
            icon_svg_color_from_values(Some("light"), None, None),
            "#1f4f8f"
        );
        assert_eq!(
            icon_svg_color_from_values(Some("dark"), None, None),
            "#7ec8ff"
        );
        assert_eq!(
            icon_svg_color_from_values(None, Some("15;0"), None),
            "#7ec8ff"
        );
        assert_eq!(
            icon_svg_color_from_values(None, Some("0;15"), None),
            "#1f4f8f"
        );
        assert_eq!(
            icon_svg_color_from_values(Some("dark"), None, Some("ABCDEF")),
            "#abcdef"
        );
        assert_eq!(
            icon_svg_color_from_values(Some("dark"), None, Some("not-a-color")),
            "#7ec8ff"
        );
    }

    #[test]
    fn bootstrap_svg_icons_render_to_images() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" fill="currentColor" viewBox="0 0 16 16"><path d="M8 1a7 7 0 1 0 0 14A7 7 0 0 0 8 1"/></svg>"##;
        let image =
            render_bootstrap_svg_icon_with_color(svg, "#1f4f8f").expect("svg should render");
        assert!(image.width() >= 16);
        assert!(image.height() >= 16);
    }

    #[test]
    fn item_filter_matches_text_notes_priority_and_tags() {
        let item = ListItem {
            id: 1,
            list_id: Some(1),
            text: "Replace batteries".to_string(),
            is_done: Some(false),
            quantity: Some("2".to_string()),
            notes: Some("AA size".to_string()),
            tldr: None,
            due_date: None,
            due_time: None,
            reminder: None,
            reminder_time: None,
            reminder_days_before: None,
            reminder_offsets: None,
            travel_time_minutes: None,
            planned_date: None,
            planned_time: None,
            priority: Some("high".to_string()),
            progress: None,
            tags: Some(vec!["hardware".to_string()]),
            parent_item_id: None,
            depth: None,
            position: None,
            completed_at: None,
            created_at: None,
            updated_at: None,
            assigned_to: None,
            child_count: None,
            done_child_count: None,
            comment_count: None,
            color: None,
            repeat_label: None,
            image_url: None,
            image_filename: None,
            attachments: None,
        };

        assert!(item_matches_filter(&item, "batteries"));
        assert!(item_matches_filter(&item, "aa"));
        assert!(item_matches_filter(&item, "high"));
        assert!(item_matches_filter(&item, "hardware"));
        assert!(!item_matches_filter(&item, "groceries"));
    }

    #[test]
    fn tags_value_trims_and_drops_empty_tags() {
        assert_eq!(
            tags_value("foo, , bar"),
            Value::Array(vec![
                Value::String("foo".to_string()),
                Value::String("bar".to_string())
            ])
        );
    }

    #[test]
    fn editor_reminder_details_enable_reminders_by_default() {
        assert!(editor_reminder_details_provided("09:00", &[]));
        assert!(editor_reminder_details_provided("", &[0, 60]));
        assert!(!editor_reminder_details_provided("  ", &[]));
    }

    #[test]
    fn parses_iso_dates_and_rejects_invalid_dates() {
        assert_eq!(
            parse_iso_date("2026-06-14T10:00:00Z"),
            Some(SimpleDate {
                year: 2026,
                month: 6,
                day: 14,
            })
        );
        assert_eq!(parse_iso_date("2026-02-29"), None);
        assert_eq!(
            parse_iso_date("2024-02-29"),
            Some(SimpleDate {
                year: 2024,
                month: 2,
                day: 29,
            })
        );
        assert_eq!(parse_iso_date("2026-13-01"), None);
    }

    #[test]
    fn validates_due_date_editor_input() {
        assert!(valid_due_date_input(""));
        assert!(valid_due_date_input("2026-06-17"));
        assert!(valid_due_date_input("2026-06-17T09:30"));
        assert!(valid_due_date_input("2026-06-17T09:30:00Z"));
        assert!(!valid_due_date_input("morgen"));
        assert!(!valid_due_date_input("2026-02-29"));
        assert!(!valid_due_date_input("2026-06-17 later"));
        assert!(due_date_input_prefix_allowed("2026-06-"));
        assert!(!due_date_input_prefix_allowed("morgen"));
    }

    #[test]
    fn preserves_due_time_suffix_when_moving_dates() {
        let target = SimpleDate {
            year: 2026,
            month: 6,
            day: 20,
        };
        assert_eq!(
            due_date_with_preserved_time(Some("2026-06-14T08:30:00Z"), target),
            "2026-06-20T08:30:00Z"
        );
        assert_eq!(
            due_date_with_preserved_time(Some("2026-06-14 08:30"), target),
            "2026-06-20 08:30"
        );
        assert_eq!(
            due_date_with_preserved_time(Some("2026-06-14"), target),
            "2026-06-20"
        );
    }

    #[test]
    fn due_date_with_hour_delta_wraps_across_days() {
        let fallback = SimpleDate {
            year: 2026,
            month: 6,
            day: 14,
        };
        assert_eq!(
            due_date_with_hour_delta(Some("2026-06-14T23:15"), fallback, 2),
            "2026-06-15T01:15"
        );
        assert_eq!(
            due_date_with_hour_delta(Some("2026-06-14T00:15"), fallback, -2),
            "2026-06-13T22:15"
        );
        assert_eq!(
            due_date_with_hour_delta(None, fallback, 0),
            "2026-06-14T09:00"
        );
    }

    #[test]
    fn calendar_date_helpers_use_monday_first_weeks() {
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2026, 2), 28);
        assert_eq!(weekday_monday0(2026, 6, 1), 0);
        assert_eq!(weekday_monday0(2026, 6, 7), 6);
        assert_eq!(
            civil_from_days(days_from_civil(2026, 6, 14)),
            SimpleDate {
                year: 2026,
                month: 6,
                day: 14,
            }
        );
    }

    #[test]
    fn calendar_cell_widths_fit_available_width() {
        assert_eq!(calendar_cell_widths(0), [0; 7]);
        assert_eq!(calendar_cell_widths(7), [1, 1, 1, 1, 1, 1, 1]);
        assert_eq!(calendar_cell_widths(10), [2, 2, 2, 1, 1, 1, 1]);
    }

    #[test]
    fn calendar_rows_use_visible_separators() {
        assert_eq!(
            calendar_row(["Mo", "Di", "Mi", "Do", "Fr", "Sa", "So"], &[2; 7]),
            "Mo│Di│Mi│Do│Fr│Sa│So"
        );
    }

    #[test]
    fn calendar_panel_layout_switches_between_wide_and_stacked_modes() {
        let (month_wide, agenda_wide) = calendar_panel_layout(Rect::new(0, 0, 100, 20));
        assert_eq!(month_wide.y, agenda_wide.y);
        assert!(month_wide.width > agenda_wide.width);

        let (month_narrow, agenda_narrow) = calendar_panel_layout(Rect::new(0, 0, 60, 20));
        assert!(month_narrow.y < agenda_narrow.y);
        assert_eq!(month_narrow.x, agenda_narrow.x);
    }

    #[test]
    fn calendar_day_cell_labels_show_selection_today_and_overflow() {
        let date = SimpleDate {
            year: 2026,
            month: 6,
            day: 14,
        };
        assert_eq!(calendar_day_cell_label(date, true, 0, false, false), " 14 ");
        assert_eq!(calendar_day_cell_label(date, true, 3, false, true), "*143");
        assert_eq!(
            calendar_day_cell_label(date, true, 12, true, false),
            "[14+]"
        );
        assert_eq!(
            calendar_day_cell_label(date, false, 0, false, false),
            "(14)"
        );
    }

    #[test]
    fn calendar_day_selection_prefers_drag_target_and_selected_day() {
        let date = SimpleDate {
            year: 2026,
            month: 6,
            day: 14,
        };
        let other = SimpleDate {
            year: 2026,
            month: 6,
            day: 15,
        };

        assert!(calendar_day_is_selected(date, None, true, None));
        assert!(!calendar_day_is_selected(other, None, false, None));

        assert!(calendar_day_is_selected(other, Some(other), false, None));
        assert!(!calendar_day_is_selected(date, Some(other), true, None));

        assert!(calendar_day_is_selected(
            date,
            Some(other),
            false,
            Some(date)
        ));
    }

    #[test]
    fn format_iso_date_zero_pads_month_and_day() {
        assert_eq!(
            format_iso_date(SimpleDate {
                year: 2026,
                month: 6,
                day: 4,
            }),
            "2026-06-04"
        );
    }

    #[test]
    fn shifted_date_moves_across_month_boundaries() {
        assert_eq!(
            shifted_date(
                SimpleDate {
                    year: 2026,
                    month: 6,
                    day: 1,
                },
                -1,
            ),
            SimpleDate {
                year: 2026,
                month: 5,
                day: 31,
            }
        );
        assert_eq!(
            shifted_date(
                SimpleDate {
                    year: 2026,
                    month: 6,
                    day: 30,
                },
                1,
            ),
            SimpleDate {
                year: 2026,
                month: 7,
                day: 1,
            }
        );
    }

    #[test]
    fn shifted_month_preserves_day_and_clamps_month_end() {
        assert_eq!(
            shifted_month(
                SimpleDate {
                    year: 2026,
                    month: 1,
                    day: 31,
                },
                1,
            ),
            SimpleDate {
                year: 2026,
                month: 2,
                day: 28,
            }
        );
        assert_eq!(
            shifted_month(
                SimpleDate {
                    year: 2026,
                    month: 1,
                    day: 15,
                },
                -1,
            ),
            SimpleDate {
                year: 2025,
                month: 12,
                day: 15,
            }
        );
    }

    #[test]
    fn start_of_month_keeps_year_and_month() {
        assert_eq!(
            start_of_month(SimpleDate {
                year: 2026,
                month: 6,
                day: 14,
            }),
            SimpleDate {
                year: 2026,
                month: 6,
                day: 1,
            }
        );
    }

    #[test]
    fn calendar_hit_helpers_return_dates_and_items() {
        let date = SimpleDate {
            year: 2026,
            month: 6,
            day: 14,
        };
        let layout = CalendarLayout {
            title: "test".to_string(),
            month_title: "month".to_string(),
            agenda_title: "agenda".to_string(),
            month_area: Rect::new(0, 0, 40, 10),
            agenda_area: Rect::new(41, 0, 20, 10),
            month_lines: Vec::new(),
            agenda_lines: Vec::new(),
            date_hits: vec![CalendarDateHit {
                rect: Rect::new(4, 5, 8, 1),
                date,
            }],
            item_hits: vec![CalendarItemHit {
                rect: Rect::new(4, 8, 40, 1),
                item_index: 7,
            }],
        };

        assert_eq!(calendar_date_at(&layout, 6, 5), Some(date));
        assert_eq!(calendar_date_at(&layout, 20, 5), None);
        assert_eq!(calendar_item_at(&layout, 10, 8), Some(7));
        assert_eq!(calendar_item_at(&layout, 10, 9), None);
    }

    #[test]
    fn calendar_pointer_date_matches_rendered_grid_cells() {
        let content = Rect::new(0, 0, 80, 20);
        let month = SimpleDate {
            year: 2026,
            month: 6,
            day: 1,
        };

        let inner = content.inner(Margin {
            vertical: 1,
            horizontal: 1,
        });
        let (month_area, _) = calendar_panel_layout(inner);
        let month_inner = month_area.inner(Margin {
            vertical: 1,
            horizontal: 1,
        });
        let widths = calendar_cell_widths(month_inner.width.saturating_sub(6));
        let monday_row = month_inner.y.saturating_add(1);

        assert_eq!(
            calendar_pointer_date(content, month_inner.x, monday_row, month),
            Some(SimpleDate {
                year: 2026,
                month: 6,
                day: 1,
            })
        );
        assert_eq!(
            calendar_pointer_date(
                content,
                month_inner.x.saturating_add(widths[0]),
                monday_row,
                month
            ),
            None
        );
    }

    #[test]
    fn calendar_month_filter_keeps_only_visible_month_dates() {
        let month = SimpleDate {
            year: 2026,
            month: 6,
            day: 1,
        };

        assert!(same_calendar_month(
            SimpleDate {
                year: 2026,
                month: 6,
                day: 30,
            },
            month
        ));
        assert!(!same_calendar_month(
            SimpleDate {
                year: 2026,
                month: 7,
                day: 1,
            },
            month
        ));
    }

    #[test]
    fn calendar_month_agenda_groups_dates_and_undated_items() {
        let mut items = vec![
            sample_item(1, "Milk"),
            sample_item(2, "Bread"),
            sample_item(3, "Apples"),
            sample_item(4, "No date"),
        ];
        items[0].due_date = Some("2026-06-04".to_string());
        items[1].due_date = Some("2026-06-04".to_string());
        items[2].due_date = Some("2026-06-14".to_string());

        let mut dated = BTreeMap::new();
        dated.insert(
            SimpleDate {
                year: 2026,
                month: 6,
                day: 4,
            },
            vec![0, 1],
        );
        dated.insert(
            SimpleDate {
                year: 2026,
                month: 6,
                day: 14,
            },
            vec![2],
        );

        let mut out = Vec::new();
        push_calendar_month_agenda_entries(&mut out, &dated, &[3], &items, 2, 20);

        assert_eq!(out[0], ("2026-06-04".to_string(), None));
        assert_eq!(out[1], ("  Milk".to_string(), Some(0)));
        assert_eq!(out[2], ("  Bread".to_string(), Some(1)));
        assert_eq!(out[3], ("2026-06-14".to_string(), None));
        assert_eq!(out[4], ("> Apples".to_string(), Some(2)));
        assert_eq!(
            out[5],
            (
                tr_args("tui-calendar-undated", &[("count", "1".to_string())]),
                None
            )
        );
        assert_eq!(out[6], ("  No date".to_string(), Some(3)));
    }

    #[test]
    fn calendar_month_agenda_respects_max_rows() {
        let mut items = vec![sample_item(1, "Milk"), sample_item(2, "Bread")];
        items[0].due_date = Some("2026-06-04".to_string());
        items[1].due_date = Some("2026-06-04".to_string());

        let mut dated = BTreeMap::new();
        dated.insert(
            SimpleDate {
                year: 2026,
                month: 6,
                day: 4,
            },
            vec![0, 1],
        );

        let mut out = Vec::new();
        push_calendar_month_agenda_entries(&mut out, &dated, &[], &items, 0, 3);

        assert_eq!(out.len(), 3);
        assert_eq!(out[0], ("2026-06-04".to_string(), None));
        assert_eq!(out[1], ("> Milk".to_string(), Some(0)));
        assert_eq!(
            out[2],
            (
                tr_args("tui-calendar-more", &[("count", "1".to_string())]),
                None
            )
        );
    }

    #[test]
    fn centered_popup_stays_inside_small_terminals() {
        let area = Rect::new(0, 0, 20, 8);
        let (outer, inner) = centered_popup(area, 56, 94, 16, 24);

        assert!(outer.width <= area.width);
        assert!(outer.height <= area.height);
        assert!(outer.x >= area.x);
        assert!(outer.y >= area.y);
        assert!(outer.x + outer.width <= area.x + area.width);
        assert!(outer.y + outer.height <= area.y + area.height);
        assert!(inner.x >= outer.x);
        assert!(inner.y >= outer.y);
        assert!(inner.x + inner.width <= outer.x + outer.width);
        assert!(inner.y + inner.height <= outer.y + outer.height);
    }

    #[test]
    fn centered_popup_uses_requested_bounds_when_space_allows() {
        let area = Rect::new(0, 0, 120, 40);
        let (outer, _) = centered_popup(area, 56, 94, 16, 24);

        assert_eq!(outer.width, 94);
        assert_eq!(outer.height, 24);
        assert_eq!(outer.x, 13);
        assert_eq!(outer.y, 8);
    }

    #[test]
    fn editor_layout_stays_inside_small_terminals() {
        let area = Rect::new(0, 0, 40, 10);
        let layout = editor_layout(area);

        assert!(layout.outer.width <= area.width);
        assert!(layout.outer.height <= area.height);
        assert!(layout.outer.x + layout.outer.width <= area.x + area.width);
        assert!(layout.outer.y + layout.outer.height <= area.y + area.height);
    }

    #[test]
    fn beta_consent_layout_stays_inside_small_terminals() {
        let area = Rect::new(0, 0, 32, 8);
        let layout = beta_consent_layout(area);

        assert!(layout.outer.width <= area.width);
        assert!(layout.outer.height <= area.height);
        assert!(layout.outer.x + layout.outer.width <= area.x + area.width);
        assert!(layout.outer.y + layout.outer.height <= area.y + area.height);
    }

    #[test]
    fn item_rows_area_rejects_block_borders() {
        let block = Rect::new(10, 5, 20, 8);
        let rows = item_rows_area(block);

        assert!(!rect_contains(rows, 11, 5));
        assert!(!rect_contains(rows, 10, 6));
        assert!(rect_contains(rows, 11, 6));
    }

    #[test]
    fn detects_supported_image_protocols_from_terminal_env() {
        assert!(
            detected_protocol_from_env_values("alacritty", "", "", false, false, false).is_none()
        );
        assert!(matches!(
            detected_protocol_from_env_values("xterm-kitty", "", "", false, false, false),
            Some(ProtocolType::Kitty)
        ));
        assert!(matches!(
            detected_protocol_from_env_values("xterm-256color", "ghostty", "", false, false, false),
            Some(ProtocolType::Kitty)
        ));
        assert!(matches!(
            detected_protocol_from_env_values("konsole", "", "", false, false, false),
            Some(ProtocolType::Kitty)
        ));
        assert!(matches!(
            detected_protocol_from_env_values("xterm-256color", "", "", true, false, false),
            Some(ProtocolType::Kitty)
        ));
        assert!(matches!(
            detected_protocol_from_env_values("foot", "", "", false, false, false),
            Some(ProtocolType::Sixel)
        ));
        assert!(matches!(
            detected_protocol_from_env_values("xterm-sixel", "", "", false, false, false),
            Some(ProtocolType::Sixel)
        ));
        assert!(matches!(
            detected_protocol_from_env_values("xterm-256color", "", "", false, false, true),
            Some(ProtocolType::Sixel)
        ));
        assert!(matches!(
            detected_protocol_from_env_values("xterm-256color", "wezterm", "", false, false, false),
            Some(ProtocolType::Iterm2)
        ));
        assert!(matches!(
            detected_protocol_from_env_values(
                "xterm-256color",
                "iterm.app",
                "",
                false,
                false,
                false
            ),
            Some(ProtocolType::Iterm2)
        ));
        assert!(matches!(
            detected_protocol_from_env_values("xterm-256color", "", "", false, true, false),
            Some(ProtocolType::Iterm2)
        ));
        assert!(matches!(
            detected_protocol_from_env_values("vscode", "", "", false, false, false),
            Some(ProtocolType::Iterm2)
        ));
    }

    #[test]
    fn image_preference_and_debug_lines_cover_env_overrides() {
        crate::test_env::with_env_lock(|| {
            let previous_protocol = std::env::var_os(KRAMLI_TUI_IMAGE_PROTOCOL_ENV);
            let previous_images = std::env::var_os(KRAMLI_TUI_IMAGES_ENV);
            let previous_term = std::env::var_os(TERM_ENV);
            let previous_program = std::env::var_os(TERM_PROGRAM_ENV);
            let previous_lc_terminal = std::env::var_os(LC_TERMINAL_ENV);
            let previous_iterm = std::env::var_os(ITERM_SESSION_ID_ENV);

            std::env::set_var(KRAMLI_TUI_IMAGE_PROTOCOL_ENV, "none");
            assert!(matches!(
                image_protocol_preference(),
                ImageProtocolPreference::Off
            ));

            std::env::set_var(KRAMLI_TUI_IMAGE_PROTOCOL_ENV, "kitty");
            assert!(matches!(
                image_protocol_preference(),
                ImageProtocolPreference::Forced(ProtocolType::Kitty)
            ));
            std::env::set_var(KRAMLI_TUI_IMAGE_PROTOCOL_ENV, "sixel");
            assert!(matches!(
                image_protocol_preference(),
                ImageProtocolPreference::Forced(ProtocolType::Sixel)
            ));
            std::env::set_var(KRAMLI_TUI_IMAGE_PROTOCOL_ENV, "imgcat");
            assert!(matches!(
                image_protocol_preference(),
                ImageProtocolPreference::Forced(ProtocolType::Iterm2)
            ));
            std::env::set_var(KRAMLI_TUI_IMAGE_PROTOCOL_ENV, "halfblocks");
            assert!(matches!(
                image_protocol_preference(),
                ImageProtocolPreference::Forced(ProtocolType::Halfblocks)
            ));
            std::env::set_var(KRAMLI_TUI_IMAGE_PROTOCOL_ENV, "unknown");
            assert!(matches!(
                image_protocol_preference(),
                ImageProtocolPreference::Auto
            ));

            std::env::remove_var(KRAMLI_TUI_IMAGE_PROTOCOL_ENV);
            std::env::set_var(KRAMLI_TUI_IMAGES_ENV, "0");
            assert!(matches!(
                image_protocol_preference(),
                ImageProtocolPreference::Off
            ));

            std::env::set_var(TERM_ENV, "xterm-256color");
            std::env::set_var(TERM_PROGRAM_ENV, "WezTerm");
            std::env::set_var(LC_TERMINAL_ENV, "iTerm2");
            std::env::remove_var(ITERM_SESSION_ID_ENV);
            std::env::set_var(KRAMLI_TUI_IMAGE_PROTOCOL_ENV, "");
            std::env::set_var(KRAMLI_TUI_IMAGES_ENV, "");

            let lines = image_runtime_debug_lines(
                ImageProtocolPreference::Auto,
                "probe",
                &Picker::halfblocks(),
                true,
            );
            assert!(
                lines
                    .iter()
                    .any(|line| line.contains("img env protocol=(unset) images=(unset)"))
            );

            match previous_protocol {
                Some(value) => std::env::set_var(KRAMLI_TUI_IMAGE_PROTOCOL_ENV, value),
                None => std::env::remove_var(KRAMLI_TUI_IMAGE_PROTOCOL_ENV),
            }
            match previous_images {
                Some(value) => std::env::set_var(KRAMLI_TUI_IMAGES_ENV, value),
                None => std::env::remove_var(KRAMLI_TUI_IMAGES_ENV),
            }
            match previous_term {
                Some(value) => std::env::set_var(TERM_ENV, value),
                None => std::env::remove_var(TERM_ENV),
            }
            match previous_program {
                Some(value) => std::env::set_var(TERM_PROGRAM_ENV, value),
                None => std::env::remove_var(TERM_PROGRAM_ENV),
            }
            match previous_lc_terminal {
                Some(value) => std::env::set_var(LC_TERMINAL_ENV, value),
                None => std::env::remove_var(LC_TERMINAL_ENV),
            }
            match previous_iterm {
                Some(value) => std::env::set_var(ITERM_SESSION_ID_ENV, value),
                None => std::env::remove_var(ITERM_SESSION_ID_ENV),
            }
        });
    }

    #[test]
    fn build_picker_and_probe_detection_cover_auto_probe_paths() {
        crate::test_env::with_env_lock(|| {
            let previous_images = std::env::var_os(KRAMLI_TUI_IMAGES_ENV);
            let previous_term = std::env::var_os(TERM_ENV);
            let previous_program = std::env::var_os(TERM_PROGRAM_ENV);
            let previous_lc_terminal = std::env::var_os(LC_TERMINAL_ENV);
            let previous_kitty = std::env::var_os(KITTY_WINDOW_ID_ENV);
            let previous_iterm = std::env::var_os(ITERM_SESSION_ID_ENV);
            let previous_wt = std::env::var_os(WT_SESSION_ENV);

            std::env::set_var(KRAMLI_TUI_IMAGES_ENV, "1");
            std::env::set_var(TERM_ENV, "xterm-256color");
            std::env::set_var(TERM_PROGRAM_ENV, "iTerm.app");
            std::env::set_var(LC_TERMINAL_ENV, "iTerm2");

            let (_picker, inline_enabled, summary, _debug) =
                build_image_picker(ImageProtocolPreference::Auto);
            assert!(inline_enabled);
            assert!(summary.contains("probe"));

            std::env::remove_var(KRAMLI_TUI_IMAGES_ENV);
            std::env::set_var(TERM_ENV, "xterm-256color");
            std::env::set_var(TERM_PROGRAM_ENV, "wezterm");
            std::env::set_var(LC_TERMINAL_ENV, "");
            std::env::remove_var(KITTY_WINDOW_ID_ENV);
            std::env::remove_var(ITERM_SESSION_ID_ENV);
            std::env::remove_var(WT_SESSION_ENV);
            assert!(should_probe_terminal_images());

            std::env::set_var(TERM_ENV, "dumb");
            std::env::set_var(TERM_PROGRAM_ENV, "");
            std::env::remove_var(KITTY_WINDOW_ID_ENV);
            std::env::remove_var(ITERM_SESSION_ID_ENV);
            std::env::remove_var(WT_SESSION_ENV);
            assert!(!should_probe_terminal_images());

            match previous_images {
                Some(value) => std::env::set_var(KRAMLI_TUI_IMAGES_ENV, value),
                None => std::env::remove_var(KRAMLI_TUI_IMAGES_ENV),
            }
            match previous_term {
                Some(value) => std::env::set_var(TERM_ENV, value),
                None => std::env::remove_var(TERM_ENV),
            }
            match previous_program {
                Some(value) => std::env::set_var(TERM_PROGRAM_ENV, value),
                None => std::env::remove_var(TERM_PROGRAM_ENV),
            }
            match previous_lc_terminal {
                Some(value) => std::env::set_var(LC_TERMINAL_ENV, value),
                None => std::env::remove_var(LC_TERMINAL_ENV),
            }
            match previous_kitty {
                Some(value) => std::env::set_var(KITTY_WINDOW_ID_ENV, value),
                None => std::env::remove_var(KITTY_WINDOW_ID_ENV),
            }
            match previous_iterm {
                Some(value) => std::env::set_var(ITERM_SESSION_ID_ENV, value),
                None => std::env::remove_var(ITERM_SESSION_ID_ENV),
            }
            match previous_wt {
                Some(value) => std::env::set_var(WT_SESSION_ENV, value),
                None => std::env::remove_var(WT_SESSION_ENV),
            }
        });
    }

    #[test]
    fn editor_value_helpers_cover_all_fields() {
        let mut editor = EditorState {
            mode: EditorMode::Edit,
            item_id: Some(1),
            text: "text".to_string(),
            quantity: "quantity".to_string(),
            due_date: "due_date".to_string(),
            due_time: "due_time".to_string(),
            planned_date: "planned_date".to_string(),
            planned_time: "planned_time".to_string(),
            reminder: "reminder".to_string(),
            reminder_time: "reminder_time".to_string(),
            reminder_offsets: "reminder_offsets".to_string(),
            travel_time_minutes: "travel_time_minutes".to_string(),
            priority: "priority".to_string(),
            tags: "tags".to_string(),
            progress: "progress".to_string(),
            notes: "notes".to_string(),
            active_field: EditorField::Text,
        };

        let cases = vec![
            (EditorField::Text, "text"),
            (EditorField::Quantity, "quantity"),
            (EditorField::DueDate, "due_date"),
            (EditorField::DueTime, "due_time"),
            (EditorField::PlannedDate, "planned_date"),
            (EditorField::PlannedTime, "planned_time"),
            (EditorField::Reminder, "reminder"),
            (EditorField::ReminderTime, "reminder_time"),
            (EditorField::ReminderOffsets, "reminder_offsets"),
            (EditorField::TravelTimeMinutes, "travel_time_minutes"),
            (EditorField::Priority, "priority"),
            (EditorField::Tags, "tags"),
            (EditorField::Progress, "progress"),
            (EditorField::Notes, "notes"),
        ];

        for (field, expected) in cases {
            editor.active_field = field;
            assert_eq!(active_editor_value(&editor), expected);
            *active_editor_value_mut(&mut editor) = format!("{expected}+");
            assert_eq!(active_editor_value(&editor), &format!("{expected}+"));
        }
    }

    #[test]
    fn kanban_defaults_and_editor_suggestion_helpers_cover_remaining_branches() {
        let mut app = test_app();
        app.lists.clear();
        let columns = app.kanban_columns();
        assert_eq!(columns.len(), 2);
        assert!(!columns[0].is_done);
        assert!(columns[1].is_done);
        assert_eq!(app.progress_choices().len(), 2);

        let mut item = sample_item(1, "Task");
        item.tags = Some(vec!["Milk".to_string(), " milk ".to_string(), "Bread".to_string()]);
        app.items = vec![item];
        assert_eq!(app.tag_suggestions(), vec!["Bread".to_string(), "Milk".to_string()]);

        app.editor = Some(EditorState {
            mode: EditorMode::Edit,
            item_id: Some(1),
            text: "Task".to_string(),
            quantity: String::default(),
            due_date: String::default(),
            due_time: String::default(),
            planned_date: String::default(),
            planned_time: String::default(),
            reminder: tr("label-off"),
            reminder_time: String::default(),
            reminder_offsets: String::default(),
            travel_time_minutes: String::default(),
            priority: String::default(),
            tags: "Br".to_string(),
            progress: tr("tui-kanban-open"),
            notes: String::default(),
            active_field: EditorField::Reminder,
        });
        assert!(app.apply_editor_suggestion(1));
        if let Some(editor) = app.editor.as_mut() {
            editor.active_field = EditorField::Progress;
        }
        assert!(app.apply_editor_suggestion(1));
        if let Some(editor) = app.editor.as_mut() {
            editor.active_field = EditorField::Tags;
        }
        assert!(app.apply_editor_suggestion(1));
    }

    #[tokio::test]
    async fn profile_result_mode_mouse_and_calendar_hour_false_paths_are_covered() {
        let mut app = test_app();
        app.beta_consent_pending = false;
        let profile = Profile {
            id: Some(1),
            display_name: Some("Ada".to_string()),
            email: None,
            photo_url: None,
            lang: None,
            is_anonymous: Some(false),
            created_at: None,
            legal: Some(crate::models::ProfileLegalStatus {
                pending: vec![crate::models::ProfilePendingLegalDoc {
                    key: Some("privacy".to_string()),
                }],
            }),
            terms_accepted: Some(false),
        };
        app.apply_profile_result(Ok(profile));
        assert!(
            app.status
                .as_deref()
                .is_some_and(|value| value.contains("privacy"))
        );
        app.apply_profile_result(Err("profile failed".to_string()));
        assert_eq!(app.status.as_deref(), Some("profile failed"));

        app.mode = ViewMode::Kanban;
        app.handle_mode_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), 1, 1),
            Rect::new(0, 0, 80, 24),
            MouseButtons {
                left_down: true,
                left_drag: false,
                left_up: false,
            },
        )
        .await
        .expect("kanban mode mouse branch should be reachable");

        app.mode = ViewMode::Calendar;
        app.handle_mode_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), 1, 1),
            Rect::new(0, 0, 80, 24),
            MouseButtons {
                left_down: true,
                left_drag: false,
                left_up: false,
            },
        )
        .await
        .expect("calendar mode mouse branch should be reachable");

        app.mode = ViewMode::List;
        assert!(!app
            .move_selected_item_calendar_hours(0)
            .await
            .expect("zero delta should return false"));
        app.mode = ViewMode::Calendar;
        app.items.clear();
        assert!(!app
            .move_selected_item_calendar_hours(2)
            .await
            .expect("missing selected item should return false"));
    }

    #[tokio::test]
    async fn localized_bool_input_and_icon_marker_helpers_cover_remaining_branches() {
        crate::i18n::set_locale("de");
        assert_eq!(parse_editor_bool_input(&tr("label-on")), Some(true));
        assert_eq!(parse_editor_bool_input(&tr("label-off")), Some(false));
        crate::i18n::set_locale("en");

        let mut app = test_app();
        app.set_inline_images_enabled(true);
        app.picker.set_protocol_type(ProtocolType::Sixel);
        let mut icon_targets = Vec::new();
        let mut used_cells = 0;
        let marker = trailing_list_icon_marker(
            &mut app,
            &mut icon_targets,
            ARCHIVED_LIST_ICON,
            3,
            5,
            &mut used_cells,
        );
        assert_eq!(marker, "   ");
        assert_eq!(icon_targets.len(), 1);

        let (_picker, inline_enabled, summary, _debug) =
            build_image_picker(ImageProtocolPreference::Forced(ProtocolType::Kitty));
        assert!(inline_enabled);
        assert!(summary.contains("set=kitty"));

        tokio::task::yield_now().await;
    }

    #[tokio::test]
    async fn run_tui_entrypoint_is_reachable_under_cfg_test() {
        let _ = run_tui().await;
    }

    #[test]
    fn iterm_env_overrides_bad_kitty_probe_result() {
        assert!(matches!(
            env_override_for_probed_protocol(ProtocolType::Kitty, Some(ProtocolType::Iterm2)),
            Some(ProtocolType::Iterm2)
        ));
        assert!(matches!(
            env_override_for_probed_protocol(ProtocolType::Halfblocks, Some(ProtocolType::Iterm2)),
            Some(ProtocolType::Iterm2)
        ));
        assert!(
            env_override_for_probed_protocol(ProtocolType::Kitty, Some(ProtocolType::Kitty))
                .is_none()
        );
        assert!(
            env_override_for_probed_protocol(ProtocolType::Sixel, Some(ProtocolType::Iterm2))
                .is_none()
        );
    }

    #[test]
    fn kanban_down_moves_within_same_column() {
        let buckets = vec![vec![0, 2, 4], vec![1, 3], vec![]];

        assert_eq!(next_kanban_selection(&buckets, 0, 1), Some(2));
        assert_eq!(next_kanban_selection(&buckets, 2, 1), Some(4));
        assert_eq!(next_kanban_selection(&buckets, 4, 1), Some(4));
        assert_eq!(next_kanban_selection(&buckets, 4, -1), Some(2));
        assert_eq!(next_kanban_selection(&buckets, 999, 1), Some(2));
    }

    #[test]
    fn stepped_kanban_selection_applies_multi_step_delta_in_column() {
        let buckets = vec![vec![10, 11, 12, 13], vec![20, 21]];

        assert_eq!(stepped_kanban_selection(&buckets, 10, 3), Some(13));
        assert_eq!(stepped_kanban_selection(&buckets, 13, -2), Some(11));
        assert_eq!(stepped_kanban_selection(&buckets, 13, 99), None);
        assert_eq!(stepped_kanban_selection(&buckets, 20, 1), Some(21));
    }

    #[test]
    fn kanban_window_reserves_hint_rows_without_losing_selection() {
        let bucket = vec![0, 1, 2, 3, 4, 5, 6, 7];
        let (start, item_count, show_top, show_bottom) = kanban_window(&bucket, 6, 5);
        assert!(show_top);
        assert!(!show_bottom);
        assert_eq!(item_count, 4);
        assert!(start <= 6 && 6 < start + item_count);
    }

    #[test]
    fn next_kanban_column_selection_keeps_row_position() {
        let buckets = vec![vec![10, 11, 12], vec![20, 21], vec![], vec![30, 31, 32, 33]];

        assert_eq!(next_kanban_column_selection(&buckets, 11, 1), Some(21));
        assert_eq!(next_kanban_column_selection(&buckets, 12, 1), Some(21));
        assert_eq!(next_kanban_column_selection(&buckets, 21, 1), Some(31));
        assert_eq!(next_kanban_column_selection(&buckets, 31, -1), Some(21));
        assert_eq!(next_kanban_column_selection(&buckets, 10, -1), None);
    }

    #[test]
    fn wrapped_index_wraps_in_both_directions() {
        assert_eq!(wrapped_index(0, 1, 3), 1);
        assert_eq!(wrapped_index(2, 1, 3), 0);
        assert_eq!(wrapped_index(0, -1, 3), 2);
        assert_eq!(wrapped_index(99, 1, 3), 0);
        assert_eq!(wrapped_index(3, 2, 0), 0);
    }

    #[test]
    fn cycle_suggestion_value_handles_exact_and_prefix_matches() {
        let progress = vec![
            "Offen".to_string(),
            "In Arbeit".to_string(),
            "Erledigt".to_string(),
        ];
        assert_eq!(
            cycle_suggestion_value("In Arbeit", &progress, 1),
            Some("Erledigt".to_string())
        );
        assert_eq!(
            cycle_suggestion_value("erledigt", &progress, 1),
            Some("Offen".to_string())
        );

        let tags = vec!["Tag".to_string(), "Tasche".to_string(), "Test".to_string()];
        assert_eq!(
            cycle_suggestion_value("Ta", &tags, 1),
            Some("Tag".to_string())
        );
        assert_eq!(
            cycle_suggestion_value("Ta", &tags, -1),
            Some("Tasche".to_string())
        );
        assert_eq!(cycle_suggestion_value("xyz", &tags, 1), None);
        assert_eq!(cycle_suggestion_value("Ta", &tags, 0), None);
    }

    #[test]
    fn autocomplete_last_tag_replaces_only_last_segment() {
        let suggestions = vec![
            "Brot".to_string(),
            "Milch".to_string(),
            "Butter".to_string(),
        ];

        assert_eq!(
            autocomplete_last_tag("bro", &suggestions, 1),
            Some("Brot".to_string())
        );
        assert_eq!(
            autocomplete_last_tag("Brot, mi", &suggestions, 1),
            Some("Brot, Milch".to_string())
        );
        assert_eq!(
            autocomplete_last_tag("Brot,  mi", &suggestions, 1),
            Some("Brot,  Milch".to_string())
        );
        assert_eq!(
            autocomplete_last_tag("Brot, Milch", &suggestions, 1),
            Some("Brot, Butter".to_string())
        );
        assert_eq!(autocomplete_last_tag("Brot, xyz", &suggestions, 1), None);
    }

    #[test]
    fn wrapped_kanban_selection_cycles_within_column() {
        let buckets = vec![vec![10, 11, 12], vec![20, 21]];

        assert_eq!(stepped_kanban_selection_wrapped(&buckets, 12, 1), Some(10));
        assert_eq!(stepped_kanban_selection_wrapped(&buckets, 10, -1), Some(12));
        assert_eq!(stepped_kanban_selection_wrapped(&buckets, 10, 4), Some(11));
        assert_eq!(stepped_kanban_selection_wrapped(&buckets, 999, 1), Some(11));
        assert_eq!(
            stepped_kanban_selection_wrapped(&buckets, 999, 0),
            Some(999)
        );
        assert_eq!(stepped_kanban_selection_wrapped(&[], 0, 1), None);
    }

    #[test]
    fn key_bindings_match_shifted_default_letters() {
        let bindings = KeyBindings {
            bindings: default_key_bindings(),
        };
        let action =
            bindings.action_for_key(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT));
        assert_eq!(action, Some(FooterAction::Add));
    }

    #[test]
    fn key_binding_parser_supports_remap_tokens() {
        let binding = parse_key_binding("ctrl+x").expect("valid key binding");
        assert_eq!(binding.label, "C+X");
        assert!(binding.matches(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL,)));
        assert!(!binding.matches(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::empty())));

        let space = parse_key_binding("space").expect("space binding");
        assert_eq!(space.label, "SPC");
        assert!(space.matches(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::empty())));
    }

    #[test]
    fn auto_handoff_only_sends_current_pending_list() {
        assert!(should_send_auto_handoff(Some(42), Some(42), 42));
        assert!(!should_send_auto_handoff(Some(43), Some(42), 42));
        assert!(!should_send_auto_handoff(Some(42), Some(43), 42));
        assert!(!should_send_auto_handoff(None, Some(42), 42));
    }

    #[test]
    fn auto_handoff_env_flag_defaults_on_and_can_disable() {
        assert!(auto_handoff_enabled_from_value(None));
        assert!(auto_handoff_enabled_from_value(Some("invalid")));
        assert!(!auto_handoff_enabled_from_value(Some("0")));
        assert!(!auto_handoff_enabled_from_value(Some("no")));
        assert!(auto_handoff_enabled_from_value(Some("yes")));
    }

    #[test]
    fn parse_state_config_columns_supports_strings_and_objects() {
        let columns = parse_state_config_columns(Some(
            r#"["My day", {"name":"Done","is_done":true}, {"name":"My day"}, {"name":"Review","done":false}]"#,
        ));
        assert_eq!(columns.len(), 3);
        assert_eq!(columns[0].name, "My day");
        assert!(!columns[0].is_done);
        assert_eq!(columns[1].name, "Done");
        assert!(columns[1].is_done);
        assert_eq!(columns[2].name, "Review");
        assert!(!columns[2].is_done);
    }

    #[test]
    fn parse_state_config_columns_marks_last_done_when_missing() {
        let columns = parse_state_config_columns(Some(r#"["Open", "My day"]"#));
        assert_eq!(columns.len(), 2);
        assert!(!columns[0].is_done);
        assert!(columns[1].is_done);
    }

    #[test]
    fn legal_pending_doc_helpers_extract_profile_and_response_keys() {
        let profile = Profile {
            id: Some(1),
            display_name: None,
            email: None,
            photo_url: None,
            lang: None,
            is_anonymous: Some(false),
            created_at: None,
            legal: Some(crate::models::ProfileLegalStatus {
                pending: vec![
                    crate::models::ProfilePendingLegalDoc {
                        key: Some("agb".to_string()),
                    },
                    crate::models::ProfilePendingLegalDoc {
                        key: Some("privacy".to_string()),
                    },
                ],
            }),
            terms_accepted: Some(false),
        };
        assert_eq!(profile_pending_legal_docs(&profile), vec!["agb", "privacy"]);

        let value = serde_json::json!({
            "legal": {
                "pending": [
                    {"key": "agb"},
                    {"key": "privacy"}
                ]
            }
        });
        assert_eq!(
            pending_legal_docs_from_value(&value),
            vec!["agb", "privacy"]
        );
    }
}
