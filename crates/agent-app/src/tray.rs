//! The menu bar icon: where Wysebee Agent is seen running, opened, paused and quit.
//!
//! A template image (`icons/tray.png`, black on transparent, 18 pt tall at 2x), which
//! macOS tints for a light or dark menu bar; a click opens the menu: **Open Wysebee
//! Agent**, **Pause** / **Resume**, **Quit**. English, like the window.

use std::time::Duration;

use agent_core::Agent;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::AppHandle;

const TEMPLATE: &[u8] = include_bytes!("../icons/tray.png");

pub const OPEN: &str = "open";
pub const PAUSE: &str = "pause";
pub const QUIT: &str = "quit";
const TRAY_ID: &str = "wysebee-agent";

/// The pause item's text, and the icon's tooltip, for what the agent is doing.
pub fn pause_text(paused: bool) -> &'static str {
    if paused {
        "Resume running programs"
    } else {
        "Pause running programs"
    }
}

pub fn tooltip(state: &str, paused: bool, waiting: usize) -> String {
    match (state, paused, waiting) {
        ("paired", true, _) => "Wysebee Agent: paused".into(),
        ("paired", false, 0) => "Wysebee Agent".into(),
        ("paired", false, n) => format!("Wysebee Agent: {n} waiting for you to allow"),
        _ => "Wysebee Agent: not paired".into(),
    }
}

pub fn install(app: &AppHandle, agent: Agent) -> tauri::Result<()> {
    let first = agent.status();
    let open = MenuItem::with_id(app, OPEN, "Open Wysebee Agent", true, None::<&str>)?;
    let pause = MenuItem::with_id(app, PAUSE, pause_text(first.paused), true, None::<&str>)?;
    let quit = MenuItem::with_id(app, QUIT, "Quit Wysebee Agent", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &pause, &PredefinedMenuItem::separator(app)?, &quit])?;
    let clicked = agent.clone();
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(Image::from_bytes(TEMPLATE)?)
        .icon_as_template(true)
        .tooltip(tooltip(first.state, first.paused, first.waiting))
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            OPEN => crate::show_window(app),
            PAUSE => {
                let agent = clicked.clone();
                tauri::async_runtime::spawn(async move {
                    let paused = agent.status().paused;
                    agent.set_paused(!paused).await;
                });
            }
            QUIT => app.exit(0),
            _ => {}
        })
        .build(app)?;

    // The switch is in the window too: keep the menu's wording and the tooltip in step.
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut shown = (first.paused, tooltip(first.state, first.paused, first.waiting));
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let s = agent.status();
            let now = (s.paused, tooltip(s.state, s.paused, s.waiting));
            if now != shown {
                let _ = pause.set_text(pause_text(now.0));
                if let Some(tray) = handle.tray_by_id(TRAY_ID) {
                    let _ = tray.set_tooltip(Some(&now.1));
                }
                shown = now;
            }
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_menu_and_tooltip_say_what_the_agent_is_doing() {
        assert_eq!(pause_text(false), "Pause running programs");
        assert_eq!(pause_text(true), "Resume running programs");
        assert_eq!(tooltip("paired", false, 0), "Wysebee Agent");
        assert_eq!(tooltip("paired", false, 2), "Wysebee Agent: 2 waiting for you to allow");
        assert_eq!(tooltip("paired", true, 2), "Wysebee Agent: paused");
        assert_eq!(tooltip("waiting", false, 0), "Wysebee Agent: not paired");
    }

    #[test]
    fn the_icon_is_a_template_the_menu_bar_can_draw() {
        assert!(TEMPLATE.starts_with(b"\x89PNG\r\n\x1a\n"));
        // 18 pt at 2x: 36 px tall.
        assert_eq!(u32::from_be_bytes(TEMPLATE[20..24].try_into().unwrap()), 36);
    }
}
