# Recipes

Common workflows and advanced patterns for building applications with Kurogane.

## Choosing a frontend source

### Development server

Load a local development server during development:

```rust
use kurogane::App;

fn main() {
    App::url("http://localhost:5173").run_or_exit();
}
```

It works with Vite, React, Vue, Svelte and any HTTP server.

Generate a starter project. See [templates](templates.md) for templates from git hosts.

```bash
kurogane new --name my-app
```

Run `kurogane init` inside an existing frontend project to wrap it instead.

### Production assets

Load a bundled frontend from disk:

```rust
use kurogane::App;

fn main() {
    App::new("dist").run_or_exit();
}
```

Assets are served through the `app://app/` protocol.

### Switching between development and production

```rust
use kurogane::App;

fn main() {
    let app = if cfg!(debug_assertions) { // or anything
        App::url("http://localhost:5173")
    } else {
        App::new("dist")
    };

    app.run_or_exit();
}
```

## Loading WebAssembly modules

Kurogane serves raw WebAssembly modules through the application protocol. Performance-critical logic can move into WebAssembly without extra tooling.

### Key capabilities

* Load `.wasm` through the `app://app/` scheme
* Direct JS <-> WASM interop
* No dependency on `wasm-bindgen` or any Rust tooling baked into the runtime

### Build a module

```bash
rustc \
  --target wasm32-unknown-unknown \
  -O \
  --crate-type=cdylib \
  demo.rs \
  -o demo.wasm
```

### Required target

```bash
rustup target add wasm32-unknown-unknown
```

Place the compiled `.wasm` beside your frontend:

```text
dist/
├── index.html
└── demo.wasm
```

Then load it with `fetch()` or `WebAssembly.instantiate`.

### Notes

* Only the compiled `.wasm` is needed at runtime
* Source files are not needed in production
* Higher-level tooling works too

## Windows

### The start window

`App::window` sets how the application's first window looks and where it opens:

```rust
use kurogane::{App, WindowOptions, WindowState};

App::new("dist")
    .window(
        WindowOptions::new()
            .title("Notes")      // without it, the window takes its page's <title>
            .size(1100, 720)     // centered on the primary display
            .min_size(640, 480)
            .state(WindowState::Maximized),
    )
    .run_or_exit();
```

Sizes and positions are in density-independent pixels (DIP). They measure the window's content without the frame the system draws around it. 1100 by 720 is the page's 1100 by 720 CSS pixels at 100% zoom. On a display at 150% that is 1650 by 1080 pixels.

Centering a window and keeping it on a display count the whole window with its frame on Windows and macOS. Under X11 the window manager adds the frame after the window shows. There the content is centered.

Without `size` or `placement` the window opens at 800 by 600 and centered. A size larger than the display's work area shrinks to fit it.

A window whose options set no title takes its page's `<title>` and follows it as a browser tab does. Popups do the same. The taskbar and the window switcher show that title for every page including a remote one. Set a title with `title` to keep it fixed.

The minimum size is the smallest content size the user can drag the window down to. The states are `Normal`, `Minimized`, `Maximized`, `Fullscreen` and `Hidden`. A window opened maximized restores to its size and centered. A hidden window's browser keeps the application running as any open browser does.

Under X11 a window opened `Fullscreen` shows at its size for now; Chromium drops a state set before the window maps. A page that goes fullscreen after its window shows is not affected.

An application started with `App::start_embedded` has no start window. Giving it `App::window` fails at startup. So do options no window can have (a size of 0 or a minimum larger than the size).

### Opening a window where it was

`App::on_window_closing` hears each of the application's windows close however it closes. The user, the page, the application and Ctrl+C all count. It reports the window's name from its options and its `WindowPlacement`. Save the placement under the name and give it back next time. Here `load` and `save` read and write the application's settings:

```rust
use kurogane::{App, WindowOptions};

let mut main = WindowOptions::new().name("main").size(1100, 720);
if let Some(placement) = load("main") {
    main = main.placement(placement);
}
App::new("dist")
    .window(main)
    .on_window_closing(|closing, _app| {
        if let Some(name) = closing.name() {
            save(name, closing.placement());
        }
    })
    .run_or_exit();
```

A `WindowPlacement` holds the place and size of the window's content and the window's state. It is serde data. In JSON it reads `{"x":200,"y":150,"width":900,"height":600,"state":"Maximized"}`. `load` and `save` can each be one `serde_json` call.

A maximized, minimized or fullscreen window reports the place and size it restores to. Its state is `Normal`, `Maximized` or `Fullscreen` and never `Minimized` or `Hidden`. A window minimized as it closes reports how it showed before. A window maximized and then minimized comes back maximized. The placement given back always opens a window the user sees.

Under xfwm4 a window the user maximized can report its maximized place and size as `Normal`. xfwm4 sends a maximize's geometry before its state.

The hook runs once for the start window and once for each other application window. It runs on the UI thread before the last window ends the application. Popups and DevTools are not reported. Writing a small file there is fine. Anything slow is not.

`placement` opens the window as it says. The window stays where it was unless no display shows that place anymore. Then it moves onto the nearest display at its size. A window larger than that display's work area shrinks to fit it. Wayland lets no application place its windows. There only the size applies.

`placement` decides the size over `size`. Keep `size` for the first run. `placement` also sets the state as `state` does; the later of the two calls holds.

A placement can be written out to put a window at an exact spot:

```rust
use kurogane::{WindowOptions, WindowPlacement, WindowState};

let options = WindowOptions::new().placement(WindowPlacement {
    x: 200,
    y: 150,
    width: 900,
    height: 600,
    state: WindowState::Normal,
});
```

### Naming windows

A name is the application's handle on a window and is never shown. `WindowOptions::name` gives it. `App::on_window_closing` reports it. `AppHandle::find_window_by_name` finds the open window by it.

One open window holds a name at a time. `create_window` with the name of a window still open fails with `RuntimeError::WindowNameTaken`. The error names the window that holds it. The name is free again once that window's close is reported. A window opened without a name reports none. So does a window a page opens.

### The window class on Linux

`App::window_class` sets the class of every window Kurogane opens on Linux. That is `WM_CLASS` under X11 and the `app_id` under Wayland. The desktop groups an application's windows by it. A launcher's icon attaches to them when the application's desktop entry names the class (`StartupWMClass=com.example.notes`, or under Wayland a desktop entry named `com.example.notes.desktop`). A compositor's rules for the application match it too.

```rust
use kurogane::App;

App::new("dist").window_class("com.example.notes").run_or_exit();
```

It names the start window, every `create_window` window, popups and DevTools. Without it the class is the executable's file name. Windows and macOS have no window class and ignore it. A browser embedded in your own window lives in your window and has its class.

`kurogane bundle` writes `[app].identifier` from `kurogane.toml` into the AppImage's desktop entry as `StartupWMClass`. Give `App::window_class` the same identifier. The launcher that started the AppImage then attaches to its windows. So does one an integration tool installs. Without an identifier the entry names the executable. That is also the windows' default class.

### The window icon

`App::window_icon` gives every window Kurogane opens the application's icon as a PNG:

```rust
use kurogane::App;

App::new("dist")
    .window_icon(include_bytes!("../icon.png").as_slice())
    .run_or_exit();
```

On Windows it shows in the window's title bar, the taskbar and the window switcher. Under X11 the window manager shows it. A 256 by 256 square is plenty; the system scales it. Under Wayland the desktop takes a window's icon from the desktop entry its class names. macOS windows have no icon; the Dock shows the application's. Bytes that are not a PNG fail at startup.

### More windows

`AppInstance::create_window` opens a page in another application window with the same options:

```rust
use kurogane::{App, RuntimeError, WindowOptions};

fn main() -> Result<(), RuntimeError> {
    let runtime = App::new("dist").start()?;

    runtime.create_window(
        "app://app/settings.html",
        WindowOptions::new().name("settings").title("Settings").size(640, 480),
    )?;

    runtime.run()
}
```

Each browser runs as a native top-level window. `run()` returns once the last browser has closed. A hidden window's browser counts too.

Only the application creates windows. Chromium's own window and tab shortcuts (Ctrl+N and Ctrl+T) do nothing in a Kurogane window.

See:

* [kurogane-suite/scenarios/multi-window.rs](../kurogane-suite/scenarios/multi-window.rs)
* [kurogane-suite/scenarios/window-management.rs](../kurogane-suite/scenarios/window-management.rs) opens windows minimized, maximized and hidden

## Links and new windows

A page can ask for a window of its own. It does so with `window.open`, a `target="_blank"` link, a form that targets a new window or a link clicked with Ctrl (Cmd on macOS), the middle button or Shift. Kurogane decides before the window exists. Chromium's own tabbed browser window never opens. Kurogane's rules:

* A page of the application's own origin opens in an application window. That origin is `app://app` for `App::new` and the start URL's origin for `App::url`. A window the page fills in itself (`about:blank`) opens there too.
* An `http` or `https` link the user clicked opens in the system's default browser.
* Anything else is refused. That includes a script opening a website on its own, `mailto:`, `file:` and custom schemes.

`App::on_new_window` changes that per request:

```rust
use kurogane::{App, NewWindowDecision, Origin};

let sign_in = Origin::parse("https://accounts.example.com").unwrap();

App::new("dist")
    .on_new_window(move |request, _app| {
        if request.origin() == &sign_in {
            // The sign-in page reports back to the page that opened it,
            // so it has to run inside the app
            NewWindowDecision::Allow
        } else {
            NewWindowDecision::Default
        }
    })
    .run_or_exit();
```

* `Allow` opens the page in an application window whatever its origin. That page reaches only the commands and events `App::permit` grants its origin.
* `Deny` refuses the window.
* `OpenExternal` sends the URL to the system browser. It does so only for an `http` or `https` link the user clicked and refuses anything else. A page cannot make the application start another program on its own.
* `Default` leaves the request to Kurogane.

Compare origins with `request.origin()` and never URL strings. `https://accounts.example.com.evil.net` starts with `https://accounts.example.com`.

`request.kind()` says how the page asked:

* `Tab` for a `target="_blank"` link, `window.open` without features or Ctrl or Cmd
* `BackgroundTab` for the middle button
* `Popup` for `window.open` with a size or position
* `Window` for Shift
* `PictureInPicture`

Kurogane opens every kind as an application window. The kind lets a policy treat a popup differently from a link. `request.user_gesture()` says whether the user clicked or a script asked on its own.

The hook runs on the UI thread before the window exists and must not block. A hook that panics refuses the window.

Test a policy by making requests the way a page sends them and calling the hook's function:

```rust
use kurogane::{NewWindowDecision, NewWindowKind, NewWindowRequest};

fn policy(request: &NewWindowRequest) -> NewWindowDecision {
    if request.kind() == NewWindowKind::Popup && !request.user_gesture() {
        NewWindowDecision::Deny
    } else {
        NewWindowDecision::Default
    }
}

let scripted = NewWindowRequest::new("https://ads.example/", "app://app/index.html")
    .with_kind(NewWindowKind::Popup);
assert_eq!(policy(&scripted), NewWindowDecision::Deny);
```

### Where a window may go

A page can also take its own window somewhere else. A link, `location = …`, a form and a redirect along the way all do that. A window shows only what was let into it:

* The application's own origin
* Every origin the application loaded there itself (the start URL, `create_window` and `BrowserHandle::navigate`) with the redirects they lead to
* The origin `on_new_window` opened a popup to
* Every origin `App::on_navigation` let in before

Within those a page navigates freely. A site the application opened can be browsed, reloaded and navigated back. A navigation anywhere else is answered as a new window is. An `http` or `https` link the user clicked opens in the system browser. Anything else is refused and the window stays on its page. Frames inside a page are not guarded; `App::permit` decides what a frame of another origin reaches.

`App::on_navigation` changes that per navigation. A sign-in provider the page sends the user to is one case:

```rust
use kurogane::{App, NavigationDecision, Origin};

let sign_in = Origin::parse("https://accounts.example.com").unwrap();

App::url("https://app.example.com")
    .on_navigation(move |navigation, _app| {
        if navigation.origin() == &sign_in {
            // Lets the provider into this window; it sends the user back
            // to app.example.com, which the window may always show
            NavigationDecision::Allow
        } else {
            NavigationDecision::Default
        }
    })
    .run_or_exit();
```

`Allow` loads the page and lets its origin into that window from then on. `Deny`, `OpenExternal` and `Default` answer as they do for new windows. The request also gives `from_origin()`, `user_gesture()` and `is_redirect()`.

The application's own loads, back and forward navigation and frames never reach the hook. It runs on the UI thread before the navigation starts. A hook that panics refuses the navigation.

## Keyboard shortcuts and Chromium's commands

A Kurogane window runs only Chromium's page-local commands. Those are reload, find, print, zoom, editing, closing the window and DevTools. It refuses the rest (new windows and tabs, history and bookmarks). Two hooks narrow that further.

`App::on_key` sees each key press before Chromium's shortcuts and the page do. It decides where the key goes. `KeyDecision::Default` lets it go on in Chromium's own order (below). `KeyDecision::Consume` takes it. No shortcut runs and the page sees none of the key, its character or its release.

```rust
use kurogane::{App, Key, KeyDecision};

App::new("dist")
    .on_key(|key, _app| {
        // Ctrl+W (Cmd+W on macOS) does not close this window
        if key.key() == Key::Char('W') && key.modifiers().primary() {
            KeyDecision::Consume
        } else {
            KeyDecision::Default
        }
    })
    .run_or_exit();
```

The hook sees only presses. Held keys repeating count too (`repeat()`). It never sees releases. `Key` names a key by its place. Shift+W is still `Key::Char('W')`. `modifiers().primary()` is Ctrl on Windows and Linux and Cmd on macOS. Check `in_editable_field()` before taking keys the user may be typing.

Chromium's own order depends on the shortcut. Most shortcuts (Ctrl+F, Ctrl+P, Ctrl+1 and F5) reach the page first. They run only when the page does not call `preventDefault()`. A page can bind them already.

Chromium reserves the shortcuts that open, close and switch tabs and windows (Ctrl+T, Ctrl+N, Ctrl+W, Ctrl+Shift+T and Ctrl+Tab). They take the key before the page sees it. A page's own binding for one never fires. A reserved shortcut with nothing to do lets the key through to the page. Ctrl+Tab in a window of one tab does. So does Ctrl+Shift+T while no tab was closed. That changes as the session goes on.

`KeyDecision::PageFirst` gives the page a reserved shortcut first every time. The page gets the key. Chromium's shortcut runs only when the page does not call `preventDefault()`. A shortcut that runs then goes on as any other. Kurogane still refuses new tabs and windows. `on_chrome_command` is still asked about closing the window.

```rust
use kurogane::{App, Key, KeyDecision};

App::new("dist")
    .on_key(|key, _app| {
        // The page's own Ctrl+T handler runs; Chromium's runs only if the
        // page lets the key through
        if key.key() == Key::Char('T') && key.modifiers().primary() {
            KeyDecision::PageFirst
        } else {
            KeyDecision::Default
        }
    })
    .run_or_exit();
```

`PageFirst` is for shortcuts:

* Chromium drops the character of a shortcut the page left alone. A typing key answered with `PageFirst` types nothing unless the page handles its keydown. Answer it for chords such as Ctrl+T and not for every key. AltGr arrives as Ctrl+Alt on Windows.
* It hands the shortcut to the page. A page that prevents Ctrl+W keeps its window open. A page that hangs holds the key.
* Chromium reserves these shortcuts so no page can keep them. Answer `PageFirst` only for windows that show your own pages (`key.browser()`).
* The window's own close button and closing it from the application are not affected.
* A browser embedded in your own window runs no Chromium shortcuts. There `PageFirst` changes nothing but the dropped character.

`App::on_chrome_command` is asked about each command the window would run from a shortcut or the context menu. It may refuse the command. It is never asked about a command Kurogane refuses. It cannot allow one.

```rust
use kurogane::{App, ChromeCommand, CommandDecision};

App::new("dist")
    .on_chrome_command(|request, _app| match request.command() {
        // No DevTools and no reload in the shipped app
        ChromeCommand::DevTools | ChromeCommand::Reload => CommandDecision::Refuse,
        _ => CommandDecision::Default,
    })
    .run_or_exit();
```

`ChromeCommand` groups Chromium's commands by what the user asked for. Refusing `DevTools` refuses Ctrl+Shift+I, F12 and the context menu's Inspect alike. Refusing `Reload` refuses every kind of reload. A key `on_key` consumed never becomes a command. Neither does one it gave the page first that the page prevented.

Both hooks run on the UI thread and `on_key` runs for every key press. Keep them quick. A panicking `on_key` lets the key through. A panicking `on_chrome_command` refuses the command. DevTools' own windows reach neither hook. A browser embedded in your own window (`create_child_browser`) reaches `on_key`. Chromium runs no commands there and `on_chrome_command` is never asked.

## Downloads

A page can download a file. That happens for a link the server answers with an attachment, a link with a `download` attribute or a `blob:` or `data:` export. Kurogane asks the user where to save it with the system's Save As dialog. The dialog fills in the suggested name. When the user cancels nothing is saved.

A window shows one dialog at a time. A page that downloads several files asks about each in turn. Without that dialog Chromium would save every file into the Downloads folder without a word.

`App::on_download` decides per download instead. One use saves the application's own exports without asking:

```rust
use kurogane::{App, DownloadDecision, Origin};

let exports = std::env::temp_dir().join("my-app-exports");
let own = Origin::parse("app://app").unwrap();

App::new("dist")
    .on_download(move |download, _app| {
        if download.origin() == &own {
            // The app's own exports go straight into its folder
            DownloadDecision::SaveTo(exports.join(download.suggested_name()))
        } else {
            // Nothing a site brings in is saved
            DownloadDecision::Deny
        }
    })
    .run_or_exit();
```

* `SaveTo` saves at that absolute path without asking and replaces a file already there. It creates the folder when missing. Nothing is saved when that fails or the path is relative.
* `Prompt` asks the user as without a hook.
* `Deny` saves nothing.
* `Default` leaves the download to Kurogane. The user is asked.

`download.origin()` is the origin of the page the download comes from and not of the file. A page of the application's own may export a `blob:` or link to a file anywhere. `download.url()` and `download.mime_type()` describe the file.

`suggested_name()` is a file name only. Chromium has already removed any folder a page tried to put in it. Joining it onto a folder of your own stays in that folder.

A page may download several files at once without a click for each. Every one of them still reaches `on_download` or the Save As dialog. Chromium's own "download multiple files" prompt never shows.

The hook runs on the UI thread before the download starts and must not block. A hook that panics refuses the download. Downloads from DevTools never reach it and always ask the user.

A `download` link to one of the application's own files (`<a href="report.pdf" download>`) does nothing. Chromium does not download from `app://` that way. Fetch the file and download it as a `blob:` instead. That reaches the hook like any other download:

```js
const response = await fetch('report.pdf');
const link = document.createElement('a');
link.href = URL.createObjectURL(await response.blob());
link.download = 'report.pdf';
link.click();
```

## Permissions

A page asks before it uses a camera, a microphone, the screen, the location, notifications, the clipboard or another `Permission`. Without a hook every request is denied and Chromium's own prompt never shows. `App::on_permission` decides instead:

```rust
use kurogane::{App, Origin, Permission, PermissionDecision};

let own = Origin::parse("app://app").unwrap();

App::new("dist")
    .on_permission(move |request, _app| {
        let calls = request
            .permissions()
            .iter()
            .all(|kind| matches!(kind, Permission::Camera | Permission::Microphone));
        if request.origin() == &own && calls {
            PermissionDecision::Allow
        } else {
            PermissionDecision::Deny
        }
    })
    .run_or_exit();
```

`Permission` covers `Camera`, `Microphone`, `ScreenVideo`, `ScreenAudio`, `Geolocation`, `Notifications`, `ClipboardRead`, `MidiSysex`, `CameraPanTiltZoom`, `ArSession`, `VrSession`, `HandTracking` and `CapturedSurfaceControl`.

A request is granted or refused whole. A page asking for a camera and a microphone together gets both or neither. `Default` gives Kurogane's answer. That is `Deny`.

Take the request's responder and answer `Later` to ask the user first. The page waits until the responder allows or denies from any thread. A responder dropped unanswered denies:

```rust
use kurogane::{App, PermissionDecision};

App::new("dist")
    .on_permission(|request, _app| {
        let responder = request.responder();
        let asked = format!("{} wants {:?}", request.origin(), request.permissions());
        std::thread::spawn(move || {
            // Your own dialog; here, a stand-in that agrees
            println!("{asked}");
            responder.allow();
        });
        PermissionDecision::Later
    })
    .run_or_exit();
```

Chromium remembers its answer to a web site (`http` or `https`) in the profile whether it granted or denied. The site's later requests get that answer without reaching the hook. `AppHandle::forget_permissions(&origin)` makes it ask again after your policy changed or the user took a permission back.

A camera, a microphone and the screen are never remembered. Nothing is remembered for the application's own pages either. A grant that must hold after the request does not hold there. Such a page is told yes for notifications and the location but cannot show a notification or read the location.

Allowing `Permission::ScreenVideo` lets the page record the whole screen with no picker. The hook runs on the UI thread and must not block. A hook that panics denies. Requests from DevTools never reach it and are denied.

## The context menu

A right-click opens Kurogane's menu and never Chromium's. That holds in a window and in an embedded browser alike. The menu shows:

* The editing items in a text field (Undo, Redo, Cut, Copy, Paste, Paste as plain text and Select all). Chromium's spelling suggestions come first on a misspelled word.
* Copy on a selection
* Nothing elsewhere
* Inspect last in a debug build

`App::on_context_menu` edits that menu as it opens. It knows what was right-clicked (a link, an image or other media, a selection or a text field). Items of your own go to `App::on_context_menu_command` when chosen:

```rust
use kurogane::{App, MenuItem, StandardItem};

App::new("dist")
    .on_context_menu(|menu, _app| {
        let target = menu.target().clone();
        if target.link_url().is_some() {
            menu.push(MenuItem::separator());
            menu.push(MenuItem::new("copy-link", "Copy link address"));
        }
        if target.is_editable() {
            // No pasting formatted text into this app's fields
            menu.remove(StandardItem::Paste);
        }
    })
    .on_context_menu_command(|command, _app| {
        if command.id() == "copy-link" {
            println!("copy {:?}", command.target().link_url());
        }
    })
    .run_or_exit();
```

* `menu.push`, `menu.remove(StandardItem::…)`, `menu.clear()` and `menu.items_mut()` add, remove and reorder items. Those are Kurogane's own (`StandardItem`), yours (`MenuItem::new(id, label)` or an `AppItem` with `enabled` and `checked`), separators and submenus (`MenuItem::submenu`).
* A menu left empty does not show. Separators at an edge or next to another and submenus left empty are not shown.
* A standard item whose command `App::on_chrome_command` refuses is left out. It is asked about again when chosen.
* `on_context_menu_command` runs only for your items and gets the same target. An item chosen after its page went away runs nothing. That happens when the page navigated while the menu stayed open.

`menu.target()` describes what was right-clicked. It gives `origin()`, `page_url()`, `frame_url()`, `link_url()`, `media()`, `media_url()`, `selection()`, `is_editable()`, `misspelled_word()` and `position()`.

A page that draws its own menu calls `preventDefault()` on its `contextmenu` event. Kurogane's menu then does not open. Both hooks run on the UI thread and must not block. A panicking `on_context_menu` leaves Kurogane's menu as it was. DevTools' menus are not asked about.

## File choosers

A page opens a file chooser with `<input type="file">` or the File System Access pickers (`showOpenFilePicker`, `showSaveFilePicker` and `showDirectoryPicker`). Without a hook the system's chooser shows. `App::on_file_dialog` can answer instead. It can pick files the page then reads, cancel or answer later from a picker of your own:

```rust
use kurogane::{App, FileDialogDecision, FileDialogKind};

let inbox = std::env::temp_dir().join("inbox.txt");

App::new("dist")
    .on_file_dialog(move |request, _app| match request.kind() {
        // The page's import always reads the inbox
        FileDialogKind::Open => FileDialogDecision::Files(vec![inbox.clone()]),
        _ => FileDialogDecision::Default,
    })
    .run_or_exit();
```

`request.kind()` is `Open`, `OpenMultiple`, `OpenFolder` or `Save`. `request.title()` and `request.default_path()` carry what the page asked for. `request.accept()` lists what the page accepts as it wrote it (`image/*` or `.png`). `request.origin()` is the page's origin.

Take `request.responder()` and answer `FileDialogDecision::Later` to show your own picker. Call `select(files)` or `cancel()` on the responder from any thread. A responder dropped unanswered cancels.

Kurogane's own Save As for a download is not asked about; `App::on_download` decides where downloads go. DevTools' dialogs are not asked about either. The hook runs on the UI thread. A hook that panics cancels the dialog.

## Drags into an embedded browser

`App::on_drag_enter` decides a drag from another application as it enters a browser embedded in your own window. The drag may carry files from a file manager, a link or selected text. The hook runs before the page sees the drag. It sees the paths of the dragged files. The page never does. `DragDecision::Refuse` keeps the drag from the page:

```rust
use kurogane::{App, DragDecision};

App::new("dist")
    .on_drag_enter(|drag, _app| {
        let images = !drag.files().is_empty()
            && drag.files().iter().all(|file| file.extension().is_some_and(|ext| ext == "png"));
        if images { DragDecision::Allow } else { DragDecision::Refuse }
    })
    .run_or_exit();
```

`drag.files()`, `drag.link_url()` and `drag.text()` say what is dragged. `drag.origin()` is the page's origin.

CEF asks only about embedded (Alloy-style) browsers. A drag into one of Kurogane's own windows is not asked about. Its page gets the drop as a browser's would.

## Following the page's title and fullscreen

`App::on_title_change` hears every title a page gives itself. `App::on_fullscreen_change` hears it enter and leave fullscreen (`requestFullscreen()`, then `exitFullscreen()` or Escape). Both report the page's browser and window:

```rust
use kurogane::App;

App::new("dist")
    .on_title_change(|change, _app| println!("{:?} is now {:?}", change.window(), change.title()))
    .on_fullscreen_change(|change, _app| println!("fullscreen: {}", change.is_fullscreen()))
    .run_or_exit();
```

A window takes its page's title and goes fullscreen with it by itself. The hooks only tell your application so it can update its own controls. DevTools is not reported.

## One instance per profile

The application keeps its settings and browsing data between launches. Starting it again while it is open brings the existing window to the front instead of opening another one.

That helps when a launch carries something to open. A user might double-click a file, choose your application from **Open With**, click a `myapp://` link or run:

```text
myapp notes.txt
```

Kurogane sends such a launch to the copy already running. `App::on_second_instance` decides what to do with it.

A common use opens the file or link in the existing window:

```rust
use kurogane::App;

App::new("dist")
    .on_second_instance(|launch, app| {
        for arg in launch.args() {
            // Relative to where the new launch started.
            let path = match launch.working_dir() {
                Some(dir) => dir.join(arg),
                None => arg.into(),
            };

            app.broadcast_json("open-file", &path);
        }
    })
    .run_or_exit();
```

```javascript
kurogane.on("open-file", (json) => openFile(JSON.parse(json)));
```

`launch.args()` lists the new launch's arguments. `launch.working_dir()` gives the directory it started in. `launch.switch("new-window")` checks for a switch such as `--new-window`.

The hook is only about **another launch of the application**. Opening another window from your own code is separate. The application can create as many windows as it needs without `on_second_instance`.

You do not have to handle a second launch. By default starting the application again brings its existing windows to the front.

On macOS opening the app bundle while it runs activates the existing application without starting another process. The hook runs only for launches that start a new process (a launch from a terminal for example).

Two copies that must run at the same time need different profiles. Give each its own with `App::profile_id`.

### Where the profile lives

The profile holds the application's cookies, storage, permissions and caches. Kurogane keeps it in `kurogane/profiles/<id>` in the local data directory. Cache cleaners leave it alone there:

* Linux: `~/.local/share/kurogane/profiles/<id>`
* macOS: `~/Library/Application Support/kurogane/profiles/<id>`
* Windows: `%LOCALAPPDATA%\kurogane\profiles\<id>`

`<id>` is `App::profile_id` or the executable's name. A debug build adds `-dev`. `kurogane list profiles` lists them. `kurogane clean all` removes them.

`App::profile_dir` puts the profile in a directory of your choosing instead. It takes an absolute path. A portable application can keep it next to its executable:

```rust
use kurogane::App;

let exe = std::env::current_exe().expect("the executable's path");
let profile = exe.parent().expect("a folder").join("profile");
App::new("dist").profile_dir(profile).run_or_exit();
```

The directory is created when missing. It is used as given in debug builds too. A development run then shares it and hands its launch over to a copy already running there. A relative path fails at startup. So does a directory given together with `App::profile_id`.

Session cookies (those without an expiry date) persist across restarts. `App::persist_session_cookies(false)` keeps them for one session only.

## Exposing Rust commands to JavaScript

Register a command with `App::command`:

```rust
use kurogane::{App, AppHandle};
use serde_json::{Value, json};

App::url("https://example.com")
    .command("ping", |payload: Value, _: &AppHandle| {
        Ok(json!({"ok": true, "echo": payload}))
    })
    .run_or_exit();
```

The handler takes the request and the `AppHandle`. The request is any type serde can deserialize. The reply is any type serde can serialize.

Call it from JavaScript:

```javascript
const result = await window.kurogane.invoke("ping", { message: "hello" });
```

`invoke` returns a promise. Its `cancel()` rejects the promise with code `0` and marks an async handler's responder cancelled.

### Async commands

`App::async_command` answers later. The handler gets a `Responder` and resolves it from any thread:

```rust
use kurogane::{App, AppHandle, Responder};

App::new("dist")
    .async_command("sum", |numbers: Vec<i64>, responder: Responder<i64>, _: &AppHandle| {
        std::thread::spawn(move || responder.resolve(Ok(numbers.iter().sum())));
    })
    .run_or_exit();
```

A responder dropped unanswered rejects the call with code `-3`. `responder.is_cancelled()` turns true when the page cancels. A cancelled responder sends nothing.

### Binary commands

`App::binary_command` takes and returns raw bytes without JSON. A page calls it with an `ArrayBuffer` or a typed array and gets an `ArrayBuffer` back:

```rust
use kurogane::{App, AppHandle};

App::new("dist")
    .binary_command("checksum", |data: &[u8], _: &AppHandle| {
        let sum: u32 = data.iter().map(|&byte| u32::from(byte)).sum();
        Ok(sum.to_le_bytes().to_vec())
    })
    .run_or_exit();
```

```javascript
const reply = await window.kurogane.invoke("checksum", new TextEncoder().encode("hello"));
```

`App::async_binary_command` is its async form. Its handler gets a `BinaryResponder`.

### Errors

A handler fails with an `IpcError`. A string converts into one. The page's promise rejects with an `Error` whose `code` says what failed:

| Code | Meaning |
|------|---------|
| `0` | The handler reported a failure, or the page cancelled |
| `-1` | The handler panicked |
| `-2` | The request or response could not be decoded or encoded |
| `-3` | An async handler dropped its responder unanswered |
| `-4` | The page's origin may not call the command |
| `-5` | No filesystem grant of the origin covers the operation |
| `-6` | The path is outside the granted roots, denied or a link |
| `-7` | The path is malformed |
| `-8` | The file exceeds the transfer limit |

`IpcError::with_code` sends a positive code of your own (`ErrorCode::App`).

See:

* [kurogane-suite/scenarios/ipc/main.rs](../kurogane-suite/scenarios/ipc/main.rs)

## Who may call a command

By default only the application's own pages call a command. Those are the pages of `app://app` for `App::new` and of the start URL's origin for `App::url`. A page of any other origin is refused with code `-4`. That covers a page in a popup, an iframe or a window that followed a link. A sandboxed frame has an opaque origin and matches no rule.

* `App::permit(name, origins)` opens a command to the origins named.
* `App::permit_all(name)` opens a command to every origin.
* `App::permit_event(name, origins)` and `App::permit_event_all(name)` do the same for event subscriptions.
* `App::deny_unlisted()` closes every command and event without a rule. That includes the application's own pages.

```rust
use kurogane::{App, AppHandle, Origin};
use serde_json::Value;

let docs = Origin::parse("https://docs.example.com").unwrap();

App::new("dist")
    .command("version", |_: Value, _: &AppHandle| Ok(env!("CARGO_PKG_VERSION")))
    .permit("version", [docs])
    .run_or_exit();
```

Kurogane checks every request before the handler runs.

## Events

`AppHandle::broadcast_json` sends an event to every page subscribed to it. `AppHandle::broadcast` sends raw bytes the same way:

```rust
app.broadcast_json("progress", &42);
```

A page subscribes with `kurogane.on` and gets the JSON text. `kurogane.off` ends the subscription:

```javascript
const id = window.kurogane.on(
    "progress",
    (json) => update(JSON.parse(json)),
    (error) => console.warn(`refused with ${error.code}`),
);
window.kurogane.off(id);
```

Subscriptions follow the same rule as commands. A page of another origin needs `App::permit_event`. A refused subscription calls the third argument with code `-4` and logs a warning without one.

## Streaming data

A stream carries chunks both ways between a page and a Rust handler. Register a factory with `App::stream`. It makes a handler for each stream a page opens. `App::stream_h` gives the factory the `AppHandle` too.

```rust
use kurogane::{App, IpcError, StreamHandler, StreamResponder};

struct Upload {
    received: usize,
}

impl StreamHandler for Upload {
    // Accept or refuse; an Err rejects the page's openStream with its
    // message and code
    fn on_open(&mut self, metadata: &str) -> Result<(), IpcError> {
        if metadata.is_empty() {
            return Err("name the upload".into());
        }
        Ok(())
    }

    // The page holds the stream now: send, end, fail, or hand a clone of
    // the responder to a thread of your own
    fn on_opened(&mut self, responder: &StreamResponder) -> Result<(), IpcError> {
        responder.send_data(b"ready")
    }

    fn on_chunk(&mut self, data: &[u8], _: &StreamResponder) -> Result<(), IpcError> {
        self.received += data.len();
        Ok(())
    }

    fn on_end(&mut self, _: &str, responder: StreamResponder) -> Result<(), IpcError> {
        responder.end(&self.received.to_string())
    }
}

fn main() {
    App::new("frontend")
        .stream("upload", || Upload { received: 0 })
        .run_or_exit();
}
```

In the page:

```javascript
const stream = await window.kurogane.openStream("upload", "notes.txt");
stream.onData((chunk) => console.log(new TextDecoder().decode(chunk)));
stream.onEnd((result) => console.log(`${result} bytes received`));
stream.write(new TextEncoder().encode("hello"));
stream.end();
```

Handlers and sends fail with an `IpcError` as commands do. A string converts into one. The first `end` or `error` a handler sends closes the stream. Later sends fail with `stream closed`. When the page calls `end()` and `on_end` sends neither, the runtime ends the stream with `""`. The page always hears back.

Handlers run on the UI thread. A `StreamResponder` can be cloned and used from any thread. Streams follow the same access rule as commands.

## The filesystem capability

A page reaches files only through `window.kurogane.fs` and only within the grants the application gives its origin. `App::filesystem` takes the grants. A scope names the directories a page may reach and the paths it may not. A grant gives an origin capabilities over a scope:

```rust
use kurogane::capability::{Filesystem, FsAccess};
use kurogane::{App, Origin};

let root = std::env::temp_dir().join("notes");

let mut fs = Filesystem::builder();
let notes = fs.scope("notes", |scope| {
    scope.allow_directory_recursive(&root);
    scope.deny_path(root.join("private"));
});
fs.grant(
    Origin::parse("app://app").unwrap(),
    notes,
    FsAccess::READ | FsAccess::LIST | FsAccess::WRITE | FsAccess::CREATE,
);

App::new("dist").filesystem(fs).run_or_exit();
```

In the page:

```javascript
const bytes = await window.kurogane.fs.readFile("today.md");
const text = new TextDecoder().decode(bytes);
await window.kurogane.fs.writeFile("today.md", text + "\n- done");
```

Paths are absolute or relative to the origin's single allowed root. Each operation needs its capability:

| `FsAccess` | Allows |
|------------|--------|
| `READ` | `readFile` and the source of `copyFile` |
| `METADATA` | `exists` and `size` |
| `LIST` | `readDir` |
| `WRITE` | `writeFile` over an existing file |
| `CREATE` | `writeFile` of a new file, `createDir` and the target of `copyFile` |
| `DELETE` | `removeFile` and `removeDir` of an empty directory |
| `RENAME` | `renameFile` |

`FsAccess::ALL` holds every capability. Scopes take `allow_directory`, `allow_directory_recursive`, `deny_path` and `deny_glob`.

* Every allowed root must exist when the application starts.
* Deny rules always win over allow rules.
* Links, junctions and other reparse points are never followed, listed or opened.
* A denied path is refused with `-6`. The answer never reveals whether the path exists.
* `readFile` and `writeFile` move whole files of up to 64 MiB. `max_file_size` on the builder changes that. `copyFile` stays in the browser process and has no cap.
* A file with other names (hard links) is refused unless the builder allows it with `allow_hard_links(true)`.
* Grants bind pages only. The application's own Rust code is not restricted.

See:

* [kurogane-suite/scenarios/workspace/main.rs](../kurogane-suite/scenarios/workspace/main.rs)

## Reaching windows and browsers from Rust

Every hook and command gets an `AppHandle`. It works from any thread:

* **Browsers:** `browsers`, `browser_count`, `get_browser_handle`, `browser_for_window`, `browser_opener`, `browser_parent` and `children_of`
* **Windows:** `windows`, `window_ids`, `window_count`, `find_window_by_name` and `find_window_by_browser`
* **Events:** `broadcast` and `broadcast_json`
* **Permissions:** `forget_permissions`
* **Ending:** `close_all_browsers`, `close_all_windows`, `shutdown` and `should_shutdown`

A `BrowserHandle` drives one browser. It can `navigate`, `reload`, `reload_ignore_cache`, `go_back`, `go_forward`, `execute_javascript`, `show_devtools`, `close_devtools` and `close`. It reports `url`, `is_loading`, `can_go_back`, `can_go_forward` and `has_devtools`. An embedding host calls `set_bounds` and `notify_move_or_resize_started`. `set_bounds` and `has_devtools` run only on the UI thread.

`start()` returns an `AppInstance`. It opens windows (`create_window`) and embedded browsers (`create_child_browser` and `create_child_browser_with_request_context`). It runs the loop (`run`, `pump`, `should_shutdown` and `shutdown`) and gives its `handle`.

## When the Chromium runtime cannot be used

The application refuses to start when its Chromium runtime cannot be used. `RuntimeError` says which runtime failed and how to fix it:

| Error | When |
|-------|------|
| `CefNotInstalled` | No runtime of the application's CEF version is installed |
| `CefPathMissing` | `CEF_PATH` names no directory |
| `CefVersionMismatch` | The runtime is another CEF build than the one the application was built against |
| `InvalidCefRuntime` | The runtime is incomplete |

`CefVersionMismatch` and `InvalidCefRuntime` carry a `CefLocation`. Each message gives the fix for its location:

* `Bundle`: reinstall the application
* `BesideExecutable`: remove the runtime beside the executable or replace it with the right CEF
* `CefPath`: point `CEF_PATH` at the right CEF or unset it
* `Installed`: remove it and reinstall it with `kurogane install`

`run_or_exit` prints the message with its causes and exits with code 1.

## Logging

Kurogane reports what it does through [`tracing`](https://docs.rs/tracing) events. Its lifecycle and IPC detail go to `debug` and problems to `warn` and `error`. Kurogane never writes to stdout or stderr itself apart from the error `run_or_exit` prints when the application fails to start. Nothing appears until the application installs a subscriber. With `tracing-subscriber`:

```toml
[dependencies]
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```

```rust
use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::LevelFilter;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::WARN.into())
                .from_env_lossy(),
        )
        .log_internal_errors(false)
        .init();

    kurogane::App::new("dist").run_or_exit();
}
```

Warnings and errors then print by default. `RUST_LOG=kurogane=debug kurogane run` adds the detail.

Keep `log_internal_errors(false)`. The default reports a line that cannot be written on stderr instead. That happens when the application's output goes into a program that has exited (`| tee` ended by Ctrl+C). When stderr is the same closed pipe that report panics. A panic inside a CEF callback aborts the application. Any other `tracing` subscriber works the same way.

## Adding Chromium flags

Pass Chromium command-line flags at startup:

```rust
use kurogane::App;

fn main() {
    App::new("frontend")
        .chromium_flag("disable-popup-blocking")
        .run_or_exit();
}
```

Flags with values:

```rust
use kurogane::App;

fn main() {
    App::new("frontend")
        .chromium_flag_with_value("enable-blink-features", "CanvasDrawElement")
        .run_or_exit();
}
```

A flag's name may be written with or without its leading `--`. A flag you pass overrides Kurogane's own value for the same switch. `enable-features` and `disable-features` are lists instead; a value adds its features to them.

Flags enable Chromium features, diagnostics and experimental functionality.

Examples:

* [kurogane-suite/scenarios/popups/main.rs](../kurogane-suite/scenarios/popups/main.rs)
* [kurogane-suite/scenarios/css-to-shader/main.rs](../kurogane-suite/scenarios/css-to-shader/main.rs)

## GPU mode selection

`App::gpu_mode` sets how Chromium renders.

### Automatic (default)

```rust
use kurogane::{App, GpuMode};

fn main() {
    App::new("frontend")
        .gpu_mode(GpuMode::Auto)
        .run_or_exit();
}
```

Kurogane picks a backend for the current environment.

### Hardware acceleration

```rust
use kurogane::{App, GpuMode};

fn main() {
    App::new("frontend")
        .gpu_mode(GpuMode::Hardware)
        .run_or_exit();
}
```

Forces GPU acceleration.

### Software rendering

```rust
use kurogane::{App, GpuMode};

fn main() {
    App::new("frontend")
        .gpu_mode(GpuMode::Software)
        .run_or_exit();
}
```

Useful for:

* Virtual machines
* CI environments
* Remote desktop sessions

### Disable GPU acceleration

```rust
use kurogane::{App, GpuMode};

fn main() {
    App::new("frontend")
        .gpu_mode(GpuMode::Disabled)
        .run_or_exit();
}
```

Disables GPU compositing and hardware acceleration.

## Credential storage

`App::credential_storage` sets how Chromium protects cookies and saved passwords at rest.

### Platform credential store (default)

```rust
use kurogane::{App, CredentialStorage};

fn main() {
    App::new("frontend")
        .credential_storage(CredentialStorage::System)
        .run_or_exit();
}
```

The encryption key is held by the Keychain on macOS, kwallet or gnome-keyring on Linux and DPAPI on Windows.

Those stores are not always reachable. Access is granted to a specific code identity. An unsigned macOS binary is authorized again after every rebuild and raises a Keychain prompt each run. Hosts with no keyring daemon have nothing to reach at all.

### Built-in store

```rust
use kurogane::{App, CredentialStorage};

fn main() {
    App::new("frontend")
        .credential_storage(CredentialStorage::Basic)
        .run_or_exit();
}
```

Chromium falls back to a fixed built-in key. That is obfuscation and not encryption. Anyone with access to the profile directory can read what it holds.

Useful for:

* Unsigned development builds
* Containers and CI environments
* Headless hosts with no keyring daemon

Not suited to profiles holding data worth protecting.

## Custom URL schemes

Register a handler for a custom URL scheme with `App::register_scheme`. The handler takes tetsu types. Kurogane re-exports them as `kurogane::tetsu`. An application does not depend on `tetsu` itself.

```rust
use kurogane::tetsu::{Browser, Frame, Request, ResourceHandler};
use kurogane::{App, SchemeHandler, resource_handler_from_bytes};

struct VirtualFile;

impl SchemeHandler for VirtualFile {
    fn create(
        &self,
        _browser: Option<&mut Browser>,
        _frame: Option<&mut Frame>,
        _request: Option<&mut Request>,
    ) -> Option<ResourceHandler> {
        Some(resource_handler_from_bytes(
            b"hello from the data scheme".to_vec(),
            "text/plain",
            200,
        ))
    }
}

App::new("frontend")
    .register_scheme("data", VirtualFile)
    .run_or_exit();
```

The frontend can load the scheme. `data://host/...` requests reach the handler whatever their host. CEF calls `create` on the browser process's IO thread. That thread also carries every IPC message. Decide there what to answer. Leave slow work such as reading a file to the returned `ResourceHandler`'s `open` and `read`. CEF calls those on a worker thread.

Useful for:

* Virtual content that is not on disk
* Synthesized resources computed at request time

Notes:

* The built-in `app` scheme (bundled assets) is reserved and cannot be overridden.
* A page served from a custom scheme has that scheme's origin (`data://host`) and not the application's. It calls a command only once `App::permit` names its origin (see [Who may call a command](#who-may-call-a-command)).
* Scheme names start with a letter and may contain letters, digits, `+`, `-` or `.`.
* An invalid or duplicate scheme name is a configuration error. `build()` returns `RuntimeError::InvalidConfiguration` listing every problem before anything starts.
* `resource_handler_from_bytes` builds a static response from `(bytes, mime, status)`. The bytes are produced in `create` on the IO thread. Implement `ResourceHandler` yourself for anything slow or more dynamic.

## Sandbox policy selection

Kurogane runs Chromium's helper processes unsandboxed by default for speed. Enable the OS sandbox with `App::sandbox_mode`:

```rust
use kurogane::{App, SandboxMode};

fn main() {
    App::new("frontend")
        .sandbox_mode(SandboxMode::Chromium)
        .run_or_exit();
}
```

### SandboxMode::Disabled (default)

* CEF starts with `no_sandbox=1` and the platform's sandbox-disabling switches
* The fastest mode

### SandboxMode::Chromium

* No sandbox-disabling switches are passed
* Helpers run sandboxed. On Linux Chromium uses unprivileged user namespaces when the kernel allows them and the setuid `chrome-sandbox` helper otherwise.
* Every Linux runtime carries `chrome-sandbox`. Whether the helper is usable is checked at startup in `Chromium` mode only.
* Each platform has a precondition the runtime checks before CEF starts. Startup fails with instructions instead of running unsandboxed:
  * Linux: user namespaces or a usable setuid helper
  * macOS: running from a `.app` bundle
  * Windows: being loaded by CEF's sandbox bootstrap. That needs a `cdylib` target, `kurogane::sandbox_entry!` and `sandbox = true` in `kurogane.toml`. See [platforms](platforms.md#chromium-sandbox-on-windows).

### Choosing a mode

Use `SandboxMode::Chromium` for:

* Staging or production builds
* Untrusted content

Keep `SandboxMode::Disabled` (the default) for:

* Development loops where the sandbox helper is unavailable
* Containers, CI and embedded targets without a usable helper

Both apply to Views and embedded runtimes.

## Custom runtime integration

Use `start()` to integrate Kurogane into an existing event loop or application runtime. Give it a scheduler. CEF then says when it next needs `pump()`. CEF recommends that for an application that pumps it from its own loop.

```rust
use std::sync::mpsc;
use std::time::{Duration, Instant};

use kurogane::{App, PumpRequest};

// The longest the loop waits between pumps, as CEF's cefclient does
const MAX_PUMP_DELAY: Duration = Duration::from_millis(1000 / 30);

fn main() {
    // CEF may call the scheduler from any thread: it hands the loop a deadline
    let (wake, deadlines) = mpsc::channel::<Instant>();

    let runtime = App::url("https://example.com")
        .scheduler(move |request: PumpRequest| {
            let _ = wake.send(request.deadline(Instant::now()));
        })
        .start()
        .expect("Kurogane failed to initialize");

    let mut next = Instant::now();
    while !runtime.should_shutdown() {
        let now = Instant::now();
        if now >= next {
            // Until CEF asks for an earlier pump
            next = now + MAX_PUMP_DELAY;
            runtime.pump();
        }
        // Sleep until the deadline or a new request; the earliest wins
        let wait = next.saturating_duration_since(Instant::now());
        if let Ok(deadline) = deadlines.recv_timeout(wait) {
            next = next.min(deadline);
        }
    }

    runtime.shutdown();
}
```

The loop runs on the thread that started Kurogane. It pumps once the earliest deadline it has not pumped yet has passed. It never waits more than 33 ms between pumps. CEF's sample application cefclient does the same.

`should_shutdown()` turns true once the application has closed its browsers. That happens after the last window closes or after `AppHandle::shutdown()` closes them all. Keep calling `pump()` until then. Then call `AppInstance::shutdown()` to shut down CEF. An application without a loop of its own calls `run()` instead.

Useful for:

* Custom event loops
* Game engines
* Framework integrations

See:

* [docs/winit.md](winit.md) for the same loop with winit
* [kurogane-suite/winit/views_scheduler.rs](../kurogane-suite/winit/views_scheduler.rs)
* [kurogane-suite/scenarios/pump.rs](../kurogane-suite/scenarios/pump.rs) pumps every 16 ms without a scheduler (the mode CEF discourages)

## Advanced: Integrating with winit

Kurogane supports several integration strategies for `winit`:

* Polling
* Fixed-interval pumping
* Scheduler-driven pumping
* Native embedding

See [docs/winit.md](winit.md) for examples and guidance.

## Advanced: Browser delegates

Browser delegates expose browser-process lifecycle hooks.

```rust
use kurogane::App;

struct BrowserDelegate;

impl kurogane::ClientAppBrowserDelegate for BrowserDelegate {
    fn on_context_initialized(&self) {
        println!("browser context initialized");
    }
}

fn main() {
    App::url("https://example.com")
        .delegate(BrowserDelegate)
        .run_or_exit();
}
```

Useful for:

* Browser process initialization
* Chromium integration
* Diagnostics and logging

See:

* [kurogane-suite/scenarios/delegates.rs](../kurogane-suite/scenarios/delegates.rs)

## Advanced: Renderer delegates

Renderer delegates expose renderer-process lifecycle hooks.

```rust
use kurogane::App;
use kurogane::tetsu::{Browser, Frame, V8Context};

struct RendererDelegate;

impl kurogane::ClientAppRendererDelegate for RendererDelegate {
    fn on_context_created(
        &self,
        _browser: Option<&Browser>,
        _frame: Option<&Frame>,
        _context: Option<&V8Context>,
    ) {
        println!("context created");
    }
}

fn main() {
    App::url("https://example.com")
        .renderer_delegate(RendererDelegate)
        .run_or_exit();
}
```

Useful for:

* JavaScript injection
* V8 integration
* Renderer diagnostics
* Custom renderer behavior

See:

* [kurogane-suite/scenarios/delegates.rs](../kurogane-suite/scenarios/delegates.rs)
