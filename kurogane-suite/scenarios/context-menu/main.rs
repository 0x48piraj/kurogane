//! Right-click menus, checked by hand: Kurogane's own menu (the editing
//! items in a text field, Copy on a selection, nothing elsewhere, Inspect in
//! a debug build) and what an application adds to it. The hooks add items
//! by what was right-clicked and print every menu and choice; the page does
//! what each choice asks, from an event the application sends it.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use kurogane::{App, AppItem, MenuItem, StandardItem};
use serde_json::json;

/// The page's theme, which the "Dark theme" check item shows and flips.
static DARK: AtomicBool = AtomicBool::new(false);
/// The page's text size: 0 small, 1 normal, 2 large.
static SIZE: AtomicU8 = AtomicU8::new(1);

fn main() {
    kurogane_suite::logging();

    App::new("scenarios/context-menu/frontend")
        .on_context_menu(|menu, _| {
            let target = menu.target();
            println!(
                "menu on {}: link {:?}, media {:?} {:?}, selection {:?}, editable {}",
                target.origin(),
                target.link_url(),
                target.media(),
                target.media_url(),
                target.selection(),
                target.is_editable()
            );
            if target.is_editable() {
                // Kurogane's editing items stay; one of the application's after them
                menu.push(MenuItem::separator());
                menu.push(MenuItem::new("insert-greeting", "Insert a &greeting"));
            } else if target.link_url().is_some() {
                menu.items_mut()
                    .insert(0, MenuItem::new("copy-link", "Copy &link address"));
            } else if target.media_url().is_some() {
                menu.items_mut()
                    .insert(0, MenuItem::new("copy-media", "Copy &image address"));
            } else if target.selection().is_none() {
                // The page itself: a check item, a submenu of check items and
                // one of Kurogane's own, above Inspect (a debug build's)
                let size = SIZE.load(Ordering::Relaxed);
                let sizes = [
                    ("size-0", "&Small"),
                    ("size-1", "&Normal"),
                    ("size-2", "&Large"),
                ]
                .into_iter()
                .enumerate()
                .map(|(index, (id, label))| {
                    AppItem::new(id, label)
                        .checked(index == usize::from(size))
                        .into()
                });
                menu.items_mut().splice(
                    0..0,
                    [
                        AppItem::new("dark", "&Dark theme")
                            .checked(DARK.load(Ordering::Relaxed))
                            .into(),
                        MenuItem::submenu("Text &size", sizes),
                        MenuItem::separator(),
                        StandardItem::Reload.into(),
                    ],
                );
            }
        })
        .on_context_menu_command(|command, app| {
            let target = command.target();
            println!("chose {} on {}", command.id(), target.origin());
            let choice = match command.id() {
                "dark" => json!({ "dark": !DARK.fetch_xor(true, Ordering::Relaxed) }),
                "size-0" | "size-1" | "size-2" => {
                    let size = command.id()[5..].parse().unwrap_or(1);
                    SIZE.store(size, Ordering::Relaxed);
                    json!({ "size": size })
                }
                "copy-link" => json!({ "copy": target.link_url() }),
                "copy-media" => json!({ "copy": target.media_url() }),
                "insert-greeting" => json!({ "insert": "Hello from the application's menu" }),
                _ => return,
            };
            app.broadcast_json("menu-choice", &choice);
        })
        .run_or_exit();
}
