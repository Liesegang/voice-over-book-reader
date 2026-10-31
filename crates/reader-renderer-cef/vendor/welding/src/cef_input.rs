// Copyright 2026 Mark Alan Boykin
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
// SPDX-License-Identifier: MPL-2.0

use crate::surface::{
    EventModifiers, FocusDirection, KeyEvent, KeyEventKind, MouseAction, MouseButton, MouseEvent,
};
/// CEF input translation: weld surface types → cef BrowserHost send methods.
///
/// Only compiled under `cef-runtime`. All functions take `&cef::BrowserHost`
/// (the cef crate's methods are `&self`, not `&mut self`).
use cef::ImplBrowserHost;

/// Async creation and retirement legitimately have no input target. A live
/// browser without a host is still an error, rather than a missing-feature lie.
#[cfg(any(windows, test))]
pub(crate) fn with_live_input_host<B, H>(
    browser: Option<B>,
    inactive: impl Fn() -> bool,
    get_host: impl FnOnce(&B) -> Option<H>,
    deliver: impl FnOnce(&H),
) -> Result<(), crate::WeldError> {
    let Some(browser) = browser.filter(|_| !inactive()) else {
        return Ok(());
    };
    let Some(host) = get_host(&browser) else {
        if inactive() {
            return Ok(());
        }
        return Err(crate::WeldError::BrowserOp(
            "active CEF browser has no input host".into(),
        ));
    };
    if !inactive() {
        deliver(&host);
    }
    Ok(())
}

// CEF event-flag bitmasks — stable ABI values from cef_event_flags_t.
const EVENTFLAG_SHIFT_DOWN: u32 = 1 << 1;
const EVENTFLAG_CONTROL_DOWN: u32 = 1 << 2;
const EVENTFLAG_ALT_DOWN: u32 = 1 << 3;
const EVENTFLAG_LEFT_MOUSE_BUTTON: u32 = 1 << 4;
const EVENTFLAG_MIDDLE_MOUSE_BUTTON: u32 = 1 << 5;
const EVENTFLAG_RIGHT_MOUSE_BUTTON: u32 = 1 << 6;
const EVENTFLAG_COMMAND_DOWN: u32 = 1 << 7; // macOS Cmd / Windows Meta

fn modifiers(m: EventModifiers) -> u32 {
    let mut f = 0u32;
    if m.shift {
        f |= EVENTFLAG_SHIFT_DOWN;
    }
    if m.ctrl {
        f |= EVENTFLAG_CONTROL_DOWN;
    }
    if m.alt {
        f |= EVENTFLAG_ALT_DOWN;
    }
    if m.left_mouse_button {
        f |= EVENTFLAG_LEFT_MOUSE_BUTTON;
    }
    if m.middle_mouse_button {
        f |= EVENTFLAG_MIDDLE_MOUSE_BUTTON;
    }
    if m.right_mouse_button {
        f |= EVENTFLAG_RIGHT_MOUSE_BUTTON;
    }
    if m.meta {
        f |= EVENTFLAG_COMMAND_DOWN;
    }
    f
}

pub fn send_mouse(host: &cef::BrowserHost, ev: &MouseEvent) {
    let cef_ev = cef::MouseEvent {
        x: ev.x,
        y: ev.y,
        modifiers: modifiers(ev.modifiers),
    };
    match ev.action {
        MouseAction::Pressed => {
            host.send_mouse_click_event(Some(&cef_ev), cef_button(ev.button), 0, 1);
        }
        MouseAction::Released => {
            host.send_mouse_click_event(Some(&cef_ev), cef_button(ev.button), 1, 1);
        }
        MouseAction::Moved => {
            host.send_mouse_move_event(Some(&cef_ev), 0);
        }
        MouseAction::WheelScrolled { delta_x, delta_y } => {
            host.send_mouse_wheel_event(Some(&cef_ev), delta_x, delta_y);
        }
    }
}

#[rustfmt::skip]
fn cef_button(b: MouseButton) -> cef::MouseButtonType {
    match b {
        MouseButton::Left   => cef::MouseButtonType::LEFT,
        MouseButton::Middle => cef::MouseButtonType::MIDDLE,
        MouseButton::Right  => cef::MouseButtonType::RIGHT,
    }
}

#[rustfmt::skip]
pub fn send_key(host: &cef::BrowserHost, ev: &KeyEvent) {
    let type_ = match ev.kind {
        KeyEventKind::RawKeyDown => cef::KeyEventType::RAWKEYDOWN,
        KeyEventKind::KeyDown    => cef::KeyEventType::KEYDOWN,
        KeyEventKind::KeyUp      => cef::KeyEventType::KEYUP,
        KeyEventKind::Char       => cef::KeyEventType::CHAR,
    };
    let ch = ev.character.map(|c| c as u16).unwrap_or(0);
    let cef_ev = cef::KeyEvent {
        type_,
        modifiers: modifiers(ev.modifiers),
        windows_key_code: ev.windows_key_code,
        native_key_code: ev.native_key_code,
        character: ch,
        unmodified_character: ch,
        ..cef::KeyEvent::default()
    };
    host.send_key_event(Some(&cef_ev));
}

/// Give input focus to the browser. Direction is unused by CEF's `set_focus`;
/// use `send_keyboard_input` with Tab/Shift-Tab for in-page focus cycling.
pub fn set_focus(host: &cef::BrowserHost, _direction: FocusDirection) {
    host.set_focus(1);
}

#[cfg(test)]
mod lifecycle_tests {
    use super::with_live_input_host;
    use std::cell::Cell;

    #[test]
    fn mouse_events_reach_a_live_host_but_not_pending_or_retired_browsers() {
        let delivered = Cell::new(0);
        with_live_input_host(
            Some(7),
            || false,
            |browser| Some(*browser),
            |host| delivered.set(*host),
        )
        .unwrap();
        assert_eq!(
            delivered.get(),
            7,
            "live mouse input must still be delivered"
        );
        with_live_input_host(
            None::<u8>,
            || false,
            |_| -> Option<u8> { panic!("creation is pending") },
            |_| panic!("no host yet"),
        )
        .unwrap();
        with_live_input_host(
            Some(7),
            || true,
            |_| -> Option<u8> { panic!("retired host must not be queried") },
            |_| panic!("closed input target"),
        )
        .unwrap();
    }

    #[test]
    fn closing_during_host_lookup_is_safe_but_a_missing_live_host_is_an_error() {
        for host_exists in [false, true] {
            let closing = Cell::new(false);
            with_live_input_host(
                Some(()),
                || closing.get(),
                |_| {
                    closing.set(true);
                    host_exists.then_some(())
                },
                |_| panic!("a host retired during lookup must not receive input"),
            )
            .unwrap();
        }
        let error = with_live_input_host(Some(()), || false, |_| None::<()>, |_| panic!("no host"))
            .unwrap_err();
        assert!(
            matches!(error, crate::WeldError::BrowserOp(message) if message.contains("active CEF browser"))
        );
    }
}
