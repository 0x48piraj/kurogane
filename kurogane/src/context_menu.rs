//! The menu a right-click opens, and what its items do.
//!
//! Chromium builds a different menu in each kind of browser. A window
//! (Chrome style) gets Chrome's own: Back, Reload, Save as, Print, Cast,
//! View page source and Inspect on the page; a link's "Open in new tab, new
//! window, incognito window"; an image's "Search image with Google"; a text
//! field's "Import passwords" and spell-check settings. Chrome runs those
//! items itself, past Kurogane's command policy (`crate::chrome_commands`):
//! "View page source" and "Import passwords" open a whole Chromium browser
//! with an address bar in the application's profile, "Cast" looks for
//! devices on the network, the Google items hand the page's text or image
//! to Google. An embedded browser (Alloy style) gets CEF's small menu
//! instead.
//!
//! Kurogane builds one menu of its own for both, from typed items: by
//! default the editing items in a text field, with Chromium's spelling
//! suggestions on a misspelled word, Copy on a selection, nothing
//! elsewhere, and Inspect in a debug build. The application's
//! [`App::on_context_menu`](crate::App::on_context_menu) hook edits it, and
//! [`App::on_context_menu_command`](crate::App::on_context_menu_command)
//! hears which of the application's own items was chosen. A standard item
//! whose command [`App::on_chrome_command`](crate::App::on_chrome_command)
//! refuses is left out of the menu, and asked about again when chosen.

use std::ffi::{CStr, c_int};
use std::fmt::Write as _;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::OnceLock;

use tetsu::sys::cef_context_menu_edit_state_flags_t as EditFlags;
use tetsu::sys::cef_menu_id_t as MenuId;
use tetsu::*;
use tracing::{debug, error, warn};

use crate::acl::Origin;
use crate::browser_registry::BrowserId;
use crate::chrome_commands::{self, ChromeCommand};
use crate::ipc::FrameId;
use crate::runtime::AppHandle;

/// One of Kurogane's items, which runs a command of Chromium's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StandardItem {
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    /// Paste without the formatting of the text pasted.
    PasteAsPlainText,
    Delete,
    SelectAll,
    /// Chromium's suggestions for the misspelled word right-clicked; nothing
    /// elsewhere.
    SpellingSuggestions,
    /// Adds the misspelled word right-clicked to the dictionary; nothing
    /// elsewhere.
    AddToDictionary,
    Back,
    Forward,
    Reload,
    Print,
    /// DevTools, inspecting the element right-clicked.
    Inspect,
}

impl StandardItem {
    /// The command it runs, as
    /// [`App::on_chrome_command`](crate::App::on_chrome_command) is asked
    /// about it; none for the spelling items.
    fn command(self) -> Option<ChromeCommand> {
        Some(match self {
            StandardItem::Undo => ChromeCommand::Undo,
            StandardItem::Redo => ChromeCommand::Redo,
            StandardItem::Cut => ChromeCommand::Cut,
            StandardItem::Copy => ChromeCommand::Copy,
            StandardItem::Paste | StandardItem::PasteAsPlainText => ChromeCommand::Paste,
            StandardItem::Delete => ChromeCommand::Delete,
            StandardItem::SelectAll => ChromeCommand::SelectAll,
            StandardItem::Back => ChromeCommand::Back,
            StandardItem::Forward => ChromeCommand::Forward,
            StandardItem::Reload => ChromeCommand::Reload,
            StandardItem::Print => ChromeCommand::Print,
            StandardItem::Inspect => ChromeCommand::DevTools,
            StandardItem::SpellingSuggestions | StandardItem::AddToDictionary => return None,
        })
    }
}

/// An item of a [`ContextMenu`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MenuItem {
    /// One of Kurogane's items.
    Standard(StandardItem),
    /// An item of the application's, which reports its choice to
    /// [`App::on_context_menu_command`](crate::App::on_context_menu_command).
    App(AppItem),
    /// A submenu.
    Submenu(Submenu),
    /// A line between groups of items. One at an edge of a menu, or next
    /// to another, is not shown.
    Separator,
}

impl MenuItem {
    /// An item of the application's: choosing it calls
    /// [`App::on_context_menu_command`](crate::App::on_context_menu_command)
    /// with `id`.
    ///
    /// In a window's menu an `&` in `label` marks the letter that chooses
    /// it from the keyboard, and `&&` shows an `&`. An embedded browser's
    /// menu, which CEF draws, takes every `&` out of a label: there a
    /// typed letter chooses the item whose label begins with it, and a
    /// label cannot show an `&`.
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        MenuItem::App(AppItem::new(id, label))
    }

    /// A line between groups of items.
    pub fn separator() -> Self {
        MenuItem::Separator
    }

    /// A submenu of `items`, under `label`.
    pub fn submenu(label: impl Into<String>, items: impl IntoIterator<Item = MenuItem>) -> Self {
        MenuItem::Submenu(Submenu::new(label, items))
    }
}

impl From<StandardItem> for MenuItem {
    fn from(item: StandardItem) -> Self {
        MenuItem::Standard(item)
    }
}

impl From<AppItem> for MenuItem {
    fn from(item: AppItem) -> Self {
        MenuItem::App(item)
    }
}

impl From<Submenu> for MenuItem {
    fn from(submenu: Submenu) -> Self {
        MenuItem::Submenu(submenu)
    }
}

/// An item of the application's in a [`ContextMenu`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppItem {
    id: String,
    label: String,
    enabled: bool,
    checked: Option<bool>,
}

impl AppItem {
    /// An item that reports `id` when chosen, shown as `label` (see
    /// [`MenuItem::new`]).
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            enabled: true,
            checked: None,
        }
    }

    /// Shown greyed out and not chosen, when `enabled` is false.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// A check item, shown checked when `checked` is true. Choosing it does
    /// not change the mark: the application shows it as it decides.
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = Some(checked);
        self
    }

    /// The id it reports when chosen.
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Whether it is checked, for a check item.
    pub fn is_checked(&self) -> Option<bool> {
        self.checked
    }
}

/// A submenu of a [`ContextMenu`]. Shown only when an item of it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submenu {
    label: String,
    items: Vec<MenuItem>,
}

impl Submenu {
    pub fn new(label: impl Into<String>, items: impl IntoIterator<Item = MenuItem>) -> Self {
        Self {
            label: label.into(),
            items: items.into_iter().collect(),
        }
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn items(&self) -> &[MenuItem] {
        &self.items
    }

    pub fn items_mut(&mut self) -> &mut Vec<MenuItem> {
        &mut self.items
    }
}

/// What kind of media was right-clicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MediaKind {
    Image,
    Video,
    Audio,
    Canvas,
    File,
    Plugin,
}

/// What a right-click was on.
#[derive(Debug, Clone)]
pub struct ContextMenuTarget {
    origin: Origin,
    page_url: String,
    frame_url: String,
    link_url: Option<String>,
    media: Option<(MediaKind, String)>,
    selection: Option<String>,
    editable: bool,
    misspelled_word: Option<String>,
    position: (i32, i32),
    browser: Option<BrowserId>,
}

impl ContextMenuTarget {
    /// The origin of the document right-clicked: the page's, or a frame's
    /// inside it. A sandboxed document's own origin is opaque, whatever its
    /// URL, and so is its origin here.
    pub fn origin(&self) -> &Origin {
        &self.origin
    }

    /// The URL of the page the browser shows.
    pub fn page_url(&self) -> &str {
        &self.page_url
    }

    /// The URL of the document right-clicked, the page's or a frame's.
    pub fn frame_url(&self) -> &str {
        &self.frame_url
    }

    /// The URL of the link right-clicked.
    pub fn link_url(&self) -> Option<&str> {
        self.link_url.as_deref()
    }

    /// The kind of media right-clicked: an image, a video.
    pub fn media(&self) -> Option<MediaKind> {
        self.media.as_ref().map(|(kind, _)| *kind)
    }

    /// The URL of the media right-clicked, when it has one.
    pub fn media_url(&self) -> Option<&str> {
        self.media
            .as_ref()
            .map(|(_, url)| url.as_str())
            .filter(|url| !url.is_empty())
    }

    /// The text selected where the right-click was.
    pub fn selection(&self) -> Option<&str> {
        self.selection.as_deref()
    }

    /// Whether the right-click was in a text field or other editable
    /// content.
    pub fn is_editable(&self) -> bool {
        self.editable
    }

    /// The misspelled word right-clicked.
    pub fn misspelled_word(&self) -> Option<&str> {
        self.misspelled_word.as_deref()
    }

    /// Where the right-click was, in the browser's view.
    pub fn position(&self) -> (i32, i32) {
        self.position
    }

    /// The browser right-clicked.
    pub fn browser(&self) -> Option<BrowserId> {
        self.browser
    }

    /// What the right-click was on, as the log says it.
    fn kinds(&self) -> String {
        let mut kinds = Vec::new();
        if self.link_url.is_some() {
            kinds.push("link");
        }
        if let Some(kind) = self.media() {
            kinds.push(match kind {
                MediaKind::Image => "image",
                MediaKind::Video => "video",
                MediaKind::Audio => "audio",
                MediaKind::Canvas => "canvas",
                MediaKind::File => "file",
                MediaKind::Plugin => "plugin",
            });
        }
        if self.editable {
            kinds.push("editable");
        }
        if self.misspelled_word.is_some() {
            kinds.push("misspelled");
        }
        if self.selection.is_some() {
            kinds.push("selection");
        }
        if kinds.is_empty() {
            kinds.push("page");
        }
        kinds.join("+")
    }
}

/// The menu a right-click opens, passed to
/// [`App::on_context_menu`](crate::App::on_context_menu) to edit.
#[derive(Debug, Clone)]
pub struct ContextMenu {
    target: ContextMenuTarget,
    items: Vec<MenuItem>,
}

impl ContextMenu {
    /// What the right-click was on.
    pub fn target(&self) -> &ContextMenuTarget {
        &self.target
    }

    pub fn items(&self) -> &[MenuItem] {
        &self.items
    }

    pub fn items_mut(&mut self) -> &mut Vec<MenuItem> {
        &mut self.items
    }

    /// Adds `item` at the end.
    pub fn push(&mut self, item: impl Into<MenuItem>) {
        self.items.push(item.into());
    }

    /// Takes `item` out, from its submenus too.
    pub fn remove(&mut self, item: StandardItem) {
        remove_from(&mut self.items, item);
    }

    /// Takes every item out: no menu shows.
    pub fn clear(&mut self) {
        self.items.clear();
    }
}

fn remove_from(items: &mut Vec<MenuItem>, item: StandardItem) {
    items.retain(|kept| *kept != MenuItem::Standard(item));
    for kept in items {
        if let MenuItem::Submenu(submenu) = kept {
            remove_from(&mut submenu.items, item);
        }
    }
}

/// An item of the application's chosen, passed to
/// [`App::on_context_menu_command`](crate::App::on_context_menu_command).
#[derive(Debug, Clone)]
pub struct ContextMenuCommand {
    id: String,
    target: ContextMenuTarget,
}

impl ContextMenuCommand {
    /// The item's id, as [`MenuItem::new`] gave it.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// What the right-click that opened the menu was on.
    pub fn target(&self) -> &ContextMenuTarget {
        &self.target
    }
}

/// Kurogane's menu for `target`: in a text field the editing items, under
/// the spelling items when Chromium has some (`spelling`); Copy on a
/// selection; nothing elsewhere; Inspect last in a debug build (`debug`).
fn default_items(target: &ContextMenuTarget, spelling: bool, debug: bool) -> Vec<MenuItem> {
    use StandardItem::*;
    let mut items: Vec<MenuItem> = Vec::new();
    if target.editable {
        if spelling {
            items.extend([
                SpellingSuggestions.into(),
                MenuItem::Separator,
                AddToDictionary.into(),
                MenuItem::Separator,
            ]);
        }
        items.extend([
            Undo.into(),
            Redo.into(),
            MenuItem::Separator,
            Cut.into(),
            Copy.into(),
            Paste.into(),
            PasteAsPlainText.into(),
            MenuItem::Separator,
            SelectAll.into(),
        ]);
    } else if target.selection.is_some() {
        items.push(Copy.into());
    }
    if debug {
        items.extend([MenuItem::Separator, Inspect.into()]);
    }
    items
}

/// `items` without what does not show: a standard item `hidden` says so
/// of, at every level, and a submenu left empty.
fn visible(items: Vec<MenuItem>, hidden: &mut impl FnMut(StandardItem) -> bool) -> Vec<MenuItem> {
    items
        .into_iter()
        .filter_map(|item| match item {
            MenuItem::Standard(standard) if hidden(standard) => None,
            MenuItem::Submenu(Submenu { label, items }) => {
                let items = visible(items, hidden);
                (!items.is_empty()).then_some(MenuItem::Submenu(Submenu { label, items }))
            }
            item => Some(item),
        })
        .collect()
}

/// `items` without the separators that would show at an edge of a menu or
/// next to another, at every level, and submenus left with none.
fn tidy(items: Vec<MenuItem>) -> Vec<MenuItem> {
    let mut kept: Vec<MenuItem> = Vec::new();
    for item in items {
        let item = match item {
            MenuItem::Submenu(Submenu { label, items }) => {
                let items = tidy(items);
                if items.is_empty() {
                    continue;
                }
                MenuItem::Submenu(Submenu { label, items })
            }
            MenuItem::Separator if matches!(kept.last(), None | Some(MenuItem::Separator)) => {
                continue;
            }
            item => item,
        };
        kept.push(item);
    }
    if kept.last() == Some(&MenuItem::Separator) {
        kept.pop();
    }
    kept
}

/// The menu as the log says it: standard items by name, the application's
/// as `app:<id>`, separators as `-`, submenus as `<label>[...]`.
fn summary(items: &[MenuItem]) -> String {
    let mut text = String::new();
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            text.push_str(", ");
        }
        match item {
            MenuItem::Standard(standard) => {
                let _ = write!(text, "{standard:?}");
            }
            MenuItem::App(app) => {
                let _ = write!(text, "app:{}", app.id);
            }
            MenuItem::Submenu(submenu) => {
                let _ = write!(text, "{}[{}]", submenu.label, summary(&submenu.items));
            }
            MenuItem::Separator => text.push('-'),
        }
    }
    if text.is_empty() {
        text.push_str("(none)");
    }
    text
}

/// How the browser's menu runs: Chrome's menu in a window, CEF's in an
/// embedded browser, each with command ids of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Style {
    Chrome,
    Alloy,
}

/// The command id of `item` in a menu of `style`. None for an item that
/// style has no command for (Alloy's Inspect, which Kurogane runs itself;
/// the spelling items, taken from Chromium's own menu) or a name the build
/// does not have.
fn command_id(style: Style, item: StandardItem) -> Option<c_int> {
    use StandardItem::*;
    match style {
        Style::Alloy => Some(
            (match item {
                Undo => MenuId::MENU_ID_UNDO,
                Redo => MenuId::MENU_ID_REDO,
                Cut => MenuId::MENU_ID_CUT,
                Copy => MenuId::MENU_ID_COPY,
                Paste => MenuId::MENU_ID_PASTE,
                PasteAsPlainText => MenuId::MENU_ID_PASTE_MATCH_STYLE,
                Delete => MenuId::MENU_ID_DELETE,
                SelectAll => MenuId::MENU_ID_SELECT_ALL,
                Back => MenuId::MENU_ID_BACK,
                Forward => MenuId::MENU_ID_FORWARD,
                Reload => MenuId::MENU_ID_RELOAD,
                Print => MenuId::MENU_ID_PRINT,
                SpellingSuggestions | AddToDictionary | Inspect => return None,
            }) as c_int,
        ),
        Style::Chrome => chrome_commands::command_id(match item {
            Undo => c"IDC_CONTENT_CONTEXT_UNDO",
            Redo => c"IDC_CONTENT_CONTEXT_REDO",
            Cut => c"IDC_CONTENT_CONTEXT_CUT",
            Copy => c"IDC_CONTENT_CONTEXT_COPY",
            Paste => c"IDC_CONTENT_CONTEXT_PASTE",
            PasteAsPlainText => c"IDC_CONTENT_CONTEXT_PASTE_AND_MATCH_STYLE",
            Delete => c"IDC_CONTENT_CONTEXT_DELETE",
            SelectAll => c"IDC_CONTENT_CONTEXT_SELECTALL",
            Back => c"IDC_BACK",
            Forward => c"IDC_FORWARD",
            Reload => c"IDC_RELOAD",
            Print => c"IDC_PRINT",
            Inspect => c"IDC_CONTENT_CONTEXT_INSPECTELEMENT",
            SpellingSuggestions | AddToDictionary => return None,
        }),
    }
}

/// The ids of Chromium's spelling items in a menu of `style`: its
/// suggestions (and "no suggestions"), and "Add to dictionary".
fn spelling_ids(style: Style) -> (Vec<c_int>, Option<c_int>) {
    match style {
        Style::Alloy => (
            (MenuId::MENU_ID_SPELLCHECK_SUGGESTION_0 as c_int
                ..=MenuId::MENU_ID_NO_SPELLING_SUGGESTIONS as c_int)
                .collect(),
            Some(MenuId::MENU_ID_ADD_TO_DICTIONARY as c_int),
        ),
        Style::Chrome => {
            static IDS: OnceLock<(Vec<c_int>, Option<c_int>)> = OnceLock::new();
            IDS.get_or_init(|| {
                // Each by its own name: the header's _LAST is another name
                // for _4, which CEF's names do not carry
                let suggestions = [
                    c"IDC_SPELLCHECK_SUGGESTION_0",
                    c"IDC_SPELLCHECK_SUGGESTION_1",
                    c"IDC_SPELLCHECK_SUGGESTION_2",
                    c"IDC_SPELLCHECK_SUGGESTION_3",
                    c"IDC_SPELLCHECK_SUGGESTION_4",
                    c"IDC_CONTENT_CONTEXT_NO_SPELLING_SUGGESTIONS",
                ]
                .into_iter()
                .filter_map(chrome_commands::command_id)
                .collect();
                (
                    suggestions,
                    chrome_commands::command_id(c"IDC_SPELLCHECK_ADD_TO_DICTIONARY"),
                )
            })
            .clone()
        }
    }
}

/// An item of Chromium's own menu kept as it is: a spelling item, whose
/// label Chromium fills in, in a window, once the suggestions arrive.
#[derive(Debug, Clone)]
struct Kept {
    id: c_int,
    label: String,
    enabled: bool,
}

/// The spelling items of Chromium's menu `model`: its suggestions, and
/// "Add to dictionary".
fn spelling_items(model: &MenuModel, style: Style) -> (Vec<Kept>, Vec<Kept>) {
    let (suggestion_ids, add_id) = spelling_ids(style);
    let mut suggestions = Vec::new();
    let mut add = Vec::new();
    for index in 0..model.count() {
        let id = model.command_id_at(index);
        let kept = || Kept {
            id,
            label: CefString::from(&model.label_at(index)).to_string(),
            enabled: model.is_enabled_at(index) != 0,
        };
        if suggestion_ids.contains(&id) {
            suggestions.push(kept());
        } else if Some(id) == add_id {
            add.push(kept());
        }
    }
    (suggestions, add)
}

/// The label of `item`: Chromium's own, in the language it runs in, or
/// English when the build has no such string.
fn label(item: StandardItem) -> String {
    use StandardItem::*;
    let (name, english): (&CStr, &str) = match item {
        Undo => (c"IDS_CONTENT_CONTEXT_UNDO", "&Undo"),
        Redo => (c"IDS_CONTENT_CONTEXT_REDO", "&Redo"),
        Cut => (c"IDS_CONTENT_CONTEXT_CUT", "Cu&t"),
        Copy => (c"IDS_CONTENT_CONTEXT_COPY", "&Copy"),
        Paste => (c"IDS_CONTENT_CONTEXT_PASTE", "&Paste"),
        PasteAsPlainText => (
            c"IDS_CONTENT_CONTEXT_PASTE_AND_MATCH_STYLE",
            "Paste as plain text",
        ),
        Delete => (c"IDS_CONTENT_CONTEXT_DELETE", "&Delete"),
        SelectAll => (c"IDS_CONTENT_CONTEXT_SELECTALL", "Select &all"),
        Back => (c"IDS_CONTENT_CONTEXT_BACK", "&Back"),
        Forward => (c"IDS_CONTENT_CONTEXT_FORWARD", "&Forward"),
        Reload => (c"IDS_CONTENT_CONTEXT_RELOAD", "&Reload"),
        Print => (c"IDS_CONTENT_CONTEXT_PRINT", "&Print…"),
        Inspect => (c"IDS_CONTENT_CONTEXT_INSPECTELEMENT", "I&nspect"),
        SpellingSuggestions | AddToDictionary => return String::new(),
    };
    // SAFETY: `name` is a valid, null-terminated C string, only read for
    // the call.
    let id = unsafe { tetsu::sys::cef_id_for_pack_string_name(name.as_ptr()) };
    let localized = (id >= 0)
        .then(resource_bundle_get_global)
        .flatten()
        .map(|bundle| CefString::from(&bundle.localized_string(id)).to_string())
        .filter(|text| !text.is_empty());
    localized.unwrap_or_else(|| english.to_owned())
}

/// Whether `item` can run now, by what the page reports of the editing it
/// allows and the browser of its history.
fn enabled(item: StandardItem, edit: i64, browser: &Browser) -> bool {
    let can = |flag: i64| edit & flag != 0;
    match item {
        StandardItem::Undo => can(edit::CAN_UNDO),
        StandardItem::Redo => can(edit::CAN_REDO),
        StandardItem::Cut => can(edit::CAN_CUT),
        StandardItem::Copy => can(edit::CAN_COPY),
        StandardItem::Paste | StandardItem::PasteAsPlainText => can(edit::CAN_PASTE),
        StandardItem::Delete => can(edit::CAN_DELETE),
        StandardItem::SelectAll => can(edit::CAN_SELECT_ALL),
        StandardItem::Back => browser.can_go_back() != 0,
        StandardItem::Forward => browser.can_go_forward() != 0,
        _ => true,
    }
}

/// What a command id of a browser's last menu runs.
#[derive(Debug, Clone)]
enum Chosen {
    App(String),
    Standard(StandardItem),
}

/// A browser's last menu: what its command ids run, what it was opened on,
/// and in which document. It stays until the next one replaces it, since
/// Chromium may close a menu before it reports the item chosen from it.
#[derive(Debug, Clone)]
pub(crate) struct OpenMenu {
    style: Style,
    commands: Vec<(c_int, Chosen)>,
    target: ContextMenuTarget,
    document: Document,
}

/// The document a menu was opened in: the page's main frame and the frame
/// right-clicked, by CEF's identifiers, which a navigation to another
/// document replaces. A page that leaves while its menu stays open leaves
/// nothing for a late choice to act on.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Document {
    main: Option<String>,
    frame: Option<String>,
}

impl Document {
    fn of(browser: &Browser, frame: Option<&Frame>) -> Self {
        let id = |frame: &Frame| CefString::from(&frame.identifier()).to_string();
        Self {
            main: browser.main_frame().map(|main| id(&main)),
            frame: frame.map(id),
        }
    }

    /// Whether the document is still the one `browser` shows.
    fn still_in(&self, browser: &Browser) -> bool {
        let main = browser
            .main_frame()
            .map(|main| CefString::from(&main.identifier()).to_string());
        let frame_there = self.frame.as_ref().is_none_or(|id| {
            browser
                .frame_by_identifier(Some(&CefString::from(id.as_str())))
                .is_some_and(|frame| frame.is_valid() != 0)
        });
        main == self.main && frame_there
    }
}

/// The process message a renderer sends the browser when a document whose
/// own origin is opaque (a sandboxed document) gets its script context:
/// CEF's menu reports only URLs, which hide a sandbox.
pub(crate) const OPAQUE_DOCUMENT: &str = "kurogane-opaque-document";
/// ...and when that context goes.
pub(crate) const OPAQUE_DOCUMENT_GONE: &str = "kurogane-opaque-document-gone";

/// Notes, or forgets, the opaque document of `frame` in `browser`, from
/// `message`, the renderer's [`OPAQUE_DOCUMENT`] or [`OPAQUE_DOCUMENT_GONE`].
/// Returns whether `message` was one of those. A renderer that claims a
/// document is opaque only makes its menu's target more cautious.
pub(crate) fn opaque_document(
    app: &AppHandle,
    browser: &Browser,
    frame: &Frame,
    message: &ProcessMessage,
) -> bool {
    let name = CefString::from(&message.name()).to_string();
    let opaque = match name.as_str() {
        OPAQUE_DOCUMENT => true,
        OPAQUE_DOCUMENT_GONE => false,
        _ => return false,
    };
    let frame = FrameId::of(frame);
    // The guard ends with the block
    let mut reg = app.registry();
    if let Some(id) = reg.browsers.find_id_by_browser(browser)
        && let Some(state) = reg.browsers.get_mut(id)
    {
        state.opaque_documents.retain(|known| *known != frame);
        if opaque {
            state.opaque_documents.push(frame);
        }
    }
    true
}

/// Fills `model` with `items`, giving the application's items, the
/// submenus and Alloy's Inspect ids from CEF's range for an application's
/// own (`next`); notes what each command id runs in `commands`.
struct Render<'a> {
    style: Style,
    browser: &'a Browser,
    edit: i64,
    spelling: &'a (Vec<Kept>, Vec<Kept>),
    next: c_int,
    commands: Vec<(c_int, Chosen)>,
}

impl Render<'_> {
    fn user_id(&mut self) -> Option<c_int> {
        if self.next > MenuId::MENU_ID_USER_LAST as c_int {
            warn!("[context menu] more items than CEF has ids for; the rest are not shown");
            return None;
        }
        self.next += 1;
        Some(self.next - 1)
    }

    fn add(&mut self, model: &MenuModel, items: &[MenuItem]) {
        for item in items {
            match item {
                MenuItem::Separator => {
                    model.add_separator();
                }
                MenuItem::App(app) => {
                    let Some(id) = self.user_id() else { return };
                    let label = CefString::from(app.label.as_str());
                    match app.checked {
                        Some(checked) => {
                            model.add_check_item(id, Some(&label));
                            model.set_checked(id, checked as c_int);
                        }
                        None => {
                            model.add_item(id, Some(&label));
                        }
                    }
                    model.set_enabled(id, app.enabled as c_int);
                    self.commands.push((id, Chosen::App(app.id.clone())));
                }
                MenuItem::Submenu(submenu) => {
                    let Some(id) = self.user_id() else { return };
                    let label = CefString::from(submenu.label.as_str());
                    if let Some(child) = model.add_sub_menu(id, Some(&label)) {
                        self.add(&child, &submenu.items);
                    }
                }
                MenuItem::Standard(standard) => self.standard(model, *standard),
            }
        }
    }

    fn standard(&mut self, model: &MenuModel, item: StandardItem) {
        let kept = match item {
            StandardItem::SpellingSuggestions => Some(&self.spelling.0),
            StandardItem::AddToDictionary => Some(&self.spelling.1),
            _ => None,
        };
        if let Some(kept) = kept {
            for kept in kept {
                model.add_item(kept.id, Some(&CefString::from(kept.label.as_str())));
                model.set_enabled(kept.id, kept.enabled as c_int);
                self.commands.push((kept.id, Chosen::Standard(item)));
            }
            return;
        }
        let id = match command_id(self.style, item) {
            Some(id) => id,
            // Alloy has no Inspect: Kurogane runs it itself
            None if item == StandardItem::Inspect => match self.user_id() {
                Some(id) => id,
                None => return,
            },
            None => {
                warn!("[context menu] {item:?} has no command in this build; not shown");
                return;
            }
        };
        model.add_item(id, Some(&CefString::from(label(item).as_str())));
        model.set_enabled(id, enabled(item, self.edit, self.browser) as c_int);
        self.commands.push((id, Chosen::Standard(item)));
    }
}

/// The target of `params`, a right-click in `browser`.
fn target_of(params: &ContextMenuParams, browser: Option<BrowserId>) -> ContextMenuTarget {
    let text = |value: CefStringUserfree| CefString::from(&value).to_string();
    let some = |value: String| (!value.is_empty()).then_some(value);
    let page_url = text(params.page_url());
    let frame_url = text(params.frame_url());
    let origin = Origin::from_url(if frame_url.is_empty() {
        &page_url
    } else {
        &frame_url
    });
    ContextMenuTarget {
        origin,
        link_url: some(text(params.link_url())),
        media: media_kind(params).map(|kind| (kind, text(params.source_url()))),
        selection: some(text(params.selection_text())),
        editable: params.is_editable() != 0,
        misspelled_word: some(text(params.misspelled_word())),
        position: (params.xcoord(), params.ycoord()),
        page_url,
        frame_url,
        browser,
    }
}

/// The kind of media `params` was opened on.
///
/// cef-rs binds the media type as a Rust enum, which a value CEF adds
/// later could not hold, so CEF's function is called as returning the
/// integer it does (as `crate::client`'s `transition` does).
fn media_kind(params: &ContextMenuParams) -> Option<MediaKind> {
    let raw = params.get_raw();
    type GetMediaType = unsafe extern "C" fn(*mut sys::_cef_context_menu_params_t) -> i32;
    // SAFETY: `raw` is the live params CEF passed to this callback, borrowed
    // as the call's `self` like every method cef-rs calls. The slot returns
    // cef_context_menu_media_type_t, a C enum: a 32-bit integer on every
    // platform CEF supports.
    let value = unsafe {
        let get: GetMediaType = std::mem::transmute((*raw).get_media_type?);
        get(raw)
    };
    match value {
        1 => Some(MediaKind::Image),
        2 => Some(MediaKind::Video),
        3 => Some(MediaKind::Audio),
        4 => Some(MediaKind::Canvas),
        5 => Some(MediaKind::File),
        6 => Some(MediaKind::Plugin),
        _ => None,
    }
}

/// CEF's edit flags this module reads (cef_types.h,
/// cef_context_menu_edit_state_flags_t). cef-rs's own constants wrap a C
/// `int` on Windows and an `unsigned int` elsewhere, so a flag is read as
/// the `i64` either fits in; a test holds these to them on every platform.
mod edit {
    pub const CAN_UNDO: i64 = 1 << 0;
    pub const CAN_REDO: i64 = 1 << 1;
    pub const CAN_CUT: i64 = 1 << 2;
    pub const CAN_COPY: i64 = 1 << 3;
    pub const CAN_PASTE: i64 = 1 << 4;
    pub const CAN_DELETE: i64 = 1 << 5;
    pub const CAN_SELECT_ALL: i64 = 1 << 6;
}

/// The edit flags of `params` (see [`edit`]).
fn edit_flags(params: &ContextMenuParams) -> i64 {
    let flags = params.edit_state_flags();
    let flags: &EditFlags = flags.as_ref();
    i64::from(flags.0)
}

/// Builds the menu of a right-click in `browser` (`id`, the application's)
/// into Chromium's `model`, and keeps what its items run in the browser's
/// state. Runs on CEF's UI thread with no lock held; the application's
/// hook runs here.
pub(crate) fn build(
    app: &AppHandle,
    browser: &Browser,
    frame: Option<&Frame>,
    id: Option<BrowserId>,
    params: &ContextMenuParams,
    model: &MenuModel,
) {
    // An ending application opens no menu and asks no hook; checked again
    // after the hook, which may have begun the end itself
    if app.is_ending() {
        debug!("[context menu] not opened: the application is ending");
        model.clear();
        return;
    }
    let style = match browser.host().map(|host| host.runtime_style()) {
        Some(style)
            if style == RuntimeStyle::from(sys::cef_runtime_style_t::CEF_RUNTIME_STYLE_ALLOY) =>
        {
            Style::Alloy
        }
        _ => Style::Chrome,
    };
    let mut target = target_of(params, id);
    // A sandboxed document's own origin is opaque, whatever its URL says
    let opaque = frame.is_some_and(|frame| {
        let frame = FrameId::of(frame);
        id.and_then(|id| {
            app.registry()
                .browsers
                .get(id)
                .map(|state| state.opaque_documents.contains(&frame))
        })
        .unwrap_or(false)
    });
    if opaque {
        target.origin = Origin::OPAQUE;
    }
    let spelling = spelling_items(model, style);
    let has_spelling = !spelling.0.is_empty() || !spelling.1.is_empty();
    let defaults = default_items(&target, has_spelling, cfg!(debug_assertions));

    let mut menu = ContextMenu {
        target: target.clone(),
        items: defaults.clone(),
    };
    let hooks = app.hooks();
    if let Some(hook) = hooks
        .as_deref()
        .and_then(|hooks| hooks.context_menu.as_ref())
        && catch_unwind(AssertUnwindSafe(|| hook(&mut menu, app))).is_err()
    {
        error!(
            "on_context_menu panicked; the menu for the {} is Kurogane's own",
            target.kinds()
        );
        menu.items = defaults;
    }

    // What the application refuses is left out, asking once a command
    let mut asked: Vec<(ChromeCommand, bool)> = Vec::new();
    let mut hidden = |item: StandardItem| {
        let refused = item.command().is_some_and(|command| {
            match asked.iter().find(|(seen, _)| *seen == command) {
                Some(&(_, refused)) => refused,
                None => {
                    let refused = chrome_commands::refuses(app, command, id);
                    asked.push((command, refused));
                    refused
                }
            }
        });
        // The spelling items show only where Chromium has them
        refused
            || (item == StandardItem::SpellingSuggestions && spelling.0.is_empty())
            || (item == StandardItem::AddToDictionary && spelling.1.is_empty())
    };
    let items = tidy(visible(menu.items, &mut hidden));

    model.clear();
    if app.is_ending() {
        debug!("[context menu] not opened: the application is ending");
        return;
    }
    let mut render = Render {
        style,
        browser,
        edit: edit_flags(params),
        spelling: &spelling,
        next: MenuId::MENU_ID_USER_FIRST as c_int,
        commands: Vec::new(),
    };
    render.add(model, &items);
    debug!(
        "[context menu] {} at {}: {}",
        target.kinds(),
        target.origin,
        summary(&items)
    );
    let open = OpenMenu {
        style,
        commands: render.commands,
        target,
        document: Document::of(browser, frame),
    };
    if let Some(id) = id
        && let Some(state) = app.registry().browsers.get_mut(id)
    {
        state.context_menu = Some(open);
    }
}

/// Runs command `command_id` chosen from the last menu of `browser`, or
/// leaves it to Chromium. Returns whether it is handled: a standard item
/// the application refuses now, an item of the application's, Alloy's
/// Inspect, and anything Kurogane did not put in the menu are; the other
/// standard items run as Chromium runs them. UI thread, no lock held.
pub(crate) fn chosen(
    app: &AppHandle,
    browser: &Browser,
    id: Option<BrowserId>,
    command_id: c_int,
) -> bool {
    // The guard ends with the statement, before the hooks run
    let open = id.and_then(|id| app.registry().browsers.get(id)?.context_menu.clone());
    let Some(open) = open else {
        debug!("[context menu] command {command_id} of a menu Kurogane did not build; not run");
        return true;
    };
    let Some((_, chosen)) = open.commands.iter().find(|(known, _)| *known == command_id) else {
        debug!("[context menu] command {command_id} is not in the menu; not run");
        return true;
    };
    if !open.document.still_in(browser) {
        debug!(
            "[context menu] {chosen:?} chosen after the document it was opened on went away; not run"
        );
        return true;
    }
    match chosen {
        Chosen::App(item) => {
            debug!("[context menu] {item} chosen");
            let hooks = app.hooks();
            if let Some(hook) = hooks
                .as_deref()
                .and_then(|hooks| hooks.context_menu_command.as_ref())
            {
                let command = ContextMenuCommand {
                    id: item.clone(),
                    target: open.target.clone(),
                };
                if catch_unwind(AssertUnwindSafe(|| hook(&command, app))).is_err() {
                    error!("on_context_menu_command panicked on {item}");
                }
            }
            true
        }
        Chosen::Standard(item) => {
            if let Some(command) = item.command()
                && chrome_commands::refuses(app, command, id)
            {
                debug!("[context menu] {item:?} refused by the application");
                return true;
            }
            if *item == StandardItem::Inspect && open.style == Style::Alloy {
                if !app.is_ending()
                    && let Some(host) = browser.host()
                {
                    let (x, y) = open.target.position;
                    debug!("[context menu] Inspect at {x},{y}: DevTools");
                    host.show_dev_tools(None, None, None, Some(&Point { x, y }));
                }
                return true;
            }
            debug!("[context menu] {item:?} runs");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use StandardItem::*;

    fn target(editable: bool, selection: Option<&str>) -> ContextMenuTarget {
        ContextMenuTarget {
            origin: Origin::parse("app://app").unwrap(),
            page_url: "app://app/index.html".into(),
            frame_url: "app://app/index.html".into(),
            link_url: None,
            media: None,
            selection: selection.map(str::to_owned),
            editable,
            misspelled_word: None,
            position: (10, 20),
            browser: None,
        }
    }

    fn names(items: &[MenuItem]) -> String {
        summary(items)
    }

    #[test]
    fn the_default_menu_edits_copies_or_is_empty() {
        let field = default_items(&target(true, None), false, false);
        assert_eq!(
            names(&field),
            "Undo, Redo, -, Cut, Copy, Paste, PasteAsPlainText, -, SelectAll"
        );
        let misspelled = default_items(&target(true, None), true, false);
        assert_eq!(
            names(&misspelled),
            "SpellingSuggestions, -, AddToDictionary, -, Undo, Redo, -, Cut, Copy, Paste, PasteAsPlainText, -, SelectAll"
        );
        assert_eq!(
            names(&default_items(&target(false, Some("words")), false, false)),
            "Copy"
        );
        assert_eq!(
            names(&default_items(&target(false, None), false, false)),
            "(none)"
        );
        // A debug build adds Inspect, after a separator tidy drops on its own
        assert_eq!(
            names(&tidy(default_items(&target(false, None), false, true))),
            "Inspect"
        );
        assert_eq!(
            names(&default_items(&target(false, Some("words")), false, true)),
            "Copy, -, Inspect"
        );
    }

    #[test]
    fn hidden_items_leave_no_stray_separator_or_empty_submenu() {
        let items = vec![
            MenuItem::separator(),
            Copy.into(),
            MenuItem::separator(),
            MenuItem::separator(),
            MenuItem::submenu("Tools", [Inspect.into()]),
            MenuItem::new("lc", "Mine"),
            MenuItem::separator(),
            Inspect.into(),
            MenuItem::separator(),
        ];
        let shown = tidy(visible(items, &mut |item| item == Inspect));
        assert_eq!(names(&shown), "Copy, -, app:lc");
    }

    #[test]
    fn remove_takes_an_item_out_of_submenus_too() {
        let mut menu = ContextMenu {
            target: target(true, None),
            items: vec![
                Copy.into(),
                MenuItem::submenu("More", [Copy.into(), Paste.into()]),
            ],
        };
        menu.remove(Copy);
        assert_eq!(names(menu.items()), "More[Paste]");
        menu.push(AppItem::new("x", "X").checked(true));
        assert_eq!(names(menu.items()), "More[Paste], app:x");
        menu.clear();
        assert!(menu.items().is_empty());
    }

    #[test]
    fn each_standard_item_but_spelling_is_a_command_of_chromium_s() {
        for item in [
            Undo,
            Redo,
            Cut,
            Copy,
            Paste,
            PasteAsPlainText,
            Delete,
            SelectAll,
            Back,
            Forward,
            Reload,
            Print,
            Inspect,
        ] {
            assert!(item.command().is_some(), "{item:?}");
            assert!(
                command_id(Style::Alloy, item).is_some() || item == Inspect,
                "{item:?}"
            );
        }
        assert!(SpellingSuggestions.command().is_none() && AddToDictionary.command().is_none());
    }

    #[test]
    fn the_edit_flags_are_cef_s() {
        for (ours, cef) in [
            (edit::CAN_UNDO, EditFlags::CM_EDITFLAG_CAN_UNDO),
            (edit::CAN_REDO, EditFlags::CM_EDITFLAG_CAN_REDO),
            (edit::CAN_CUT, EditFlags::CM_EDITFLAG_CAN_CUT),
            (edit::CAN_COPY, EditFlags::CM_EDITFLAG_CAN_COPY),
            (edit::CAN_PASTE, EditFlags::CM_EDITFLAG_CAN_PASTE),
            (edit::CAN_DELETE, EditFlags::CM_EDITFLAG_CAN_DELETE),
            (edit::CAN_SELECT_ALL, EditFlags::CM_EDITFLAG_CAN_SELECT_ALL),
        ] {
            assert_eq!(ours, i64::from(cef.0), "{cef:?}");
        }
    }

    #[test]
    fn the_target_says_what_was_right_clicked() {
        let mut link = target(false, None);
        link.link_url = Some("app://app/next.html".into());
        assert_eq!(link.kinds(), "link");
        let mut image = target(false, Some("words"));
        image.media = Some((MediaKind::Image, String::new()));
        assert_eq!(image.kinds(), "image+selection");
        assert_eq!(image.media_url(), None);
        assert_eq!(target(false, None).kinds(), "page");
    }
}
