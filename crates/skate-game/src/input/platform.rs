//! Windows device transport. Raw signed axes/trigger bytes reach the TU3
//! converter without Bevy/gilrs deadzones or normalized-axis reconstruction.
use skate_core::input::xbox::XboxState;

pub(crate) struct DevicePacket {
    pub number: u32,
    pub state: XboxState,
    pub subtype: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DeviceError {
    Disconnected,
    State(u32),
    Capabilities(u32),
    #[cfg(not(windows))]
    UnsupportedPlatform,
}

/// Device identity is metadata; raw input is still sampled every host frame.
/// Refresh periodically as well as after errors, so hot swaps cannot leave a
/// subtype cached indefinitely even if Windows never exposes a disconnect.
#[derive(Default)]
pub(crate) struct CapabilityCache {
    value: Option<(u8, skate_vfs::time::Instant)>,
}
impl CapabilityCache {
    pub(crate) fn invalidate(&mut self) {
        self.value = None;
    }
    fn get(
        &mut self,
        now: skate_vfs::time::Instant,
        read: impl FnOnce() -> Result<u8, DeviceError>,
    ) -> Result<u8, DeviceError> {
        if let Some((subtype, expires)) = self.value {
            if now < expires {
                return Ok(subtype);
            }
        }
        self.value = None;
        let subtype = read()?;
        self.value = Some((subtype, now + skate_vfs::time::Duration::from_secs(1)));
        Ok(subtype)
    }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::mem::MaybeUninit;

    // ABI from the installed Windows SDK Xinput.h. No OS-owned pointers are
    // retained and only successful calls permit reading output storage.
    #[repr(C)]
    struct Gamepad {
        buttons: u16,
        left_trigger: u8,
        right_trigger: u8,
        left_x: i16,
        left_y: i16,
        right_x: i16,
        right_y: i16,
    }
    #[repr(C)]
    struct State {
        number: u32,
        gamepad: Gamepad,
    }
    #[repr(C)]
    struct Vibration {
        left: u16,
        right: u16,
    }
    #[repr(C)]
    struct Capabilities {
        device_type: u8,
        subtype: u8,
        flags: u16,
        gamepad: Gamepad,
        vibration: Vibration,
    }
    const _: () = assert!(size_of::<Gamepad>() == 12);
    const _: () = assert!(size_of::<State>() == 16);
    const _: () = assert!(size_of::<Capabilities>() == 20);

    #[link(name = "xinput")]
    unsafe extern "system" {
        fn XInputGetState(index: u32, state: *mut State) -> u32;
        fn XInputGetCapabilities(index: u32, flags: u32, capabilities: *mut Capabilities) -> u32;
    }

    pub(super) fn poll(
        index: u32,
        cache: &mut CapabilityCache,
    ) -> Result<DevicePacket, DeviceError> {
        let mut state = MaybeUninit::<State>::uninit();
        // SAFETY: properly aligned writable storage with the SDK's exact C ABI.
        let result = unsafe { XInputGetState(index, state.as_mut_ptr()) };
        if result != 0 {
            cache.invalidate();
        }
        if result == 1167 {
            return Err(DeviceError::Disconnected);
        }
        if result != 0 {
            return Err(DeviceError::State(result));
        }
        let subtype = cache.get(skate_vfs::time::Instant::now(), || {
            let mut capabilities = MaybeUninit::<Capabilities>::uninit();
            // SAFETY: writable storage with the SDK ABI; read only on success.
            let result = unsafe { XInputGetCapabilities(index, 1, capabilities.as_mut_ptr()) };
            if result != 0 {
                return Err(DeviceError::Capabilities(result));
            }
            Ok(unsafe { capabilities.assume_init() }.subtype)
        })?;
        // SAFETY: successful XInputGetState initialized the complete structure.
        let state = unsafe { state.assume_init() };
        Ok(DevicePacket {
            number: state.number,
            state: XboxState {
                buttons: state.gamepad.buttons,
                triggers: [state.gamepad.left_trigger, state.gamepad.right_trigger],
                left: [state.gamepad.left_x, state.gamepad.left_y],
                right: [state.gamepad.right_x, state.gamepad.right_y],
            },
            subtype,
        })
    }
}

pub(crate) fn poll_cached(
    index: usize,
    cache: &mut CapabilityCache,
) -> Result<DevicePacket, DeviceError> {
    assert!(index < 4);
    #[cfg(windows)]
    return windows::poll(index as u32, cache);
    #[cfg(target_arch = "wasm32")]
    return web::poll(index, cache);
    #[cfg(not(any(windows, target_arch = "wasm32")))]
    Err(DeviceError::UnsupportedPlatform)
}

/// Web: keyboard and browser gamepads (Bevy/gilrs) synthesize the same raw
/// XInput packet the Windows path reads. Slot 0 merges the keyboard with the
/// first gamepad; further gamepads take slots 1-3. See docs/web.md.
#[cfg(target_arch = "wasm32")]
pub(crate) mod web {
    use super::*;
    use bevy::input::gamepad::{Gamepad, GamepadAxis, GamepadButton};
    use bevy::prelude::{ButtonInput, KeyCode, Query, Res};
    use std::sync::Mutex;
    use wasm_bindgen::JsCast;

    static PADS: Mutex<([Option<XboxState>; 4], u32)> = Mutex::new(([None; 4], 0));

    pub(super) fn poll(index: usize, cache: &mut CapabilityCache) -> Result<DevicePacket, DeviceError> {
        let pads = PADS.lock().unwrap();
        match pads.0[index] {
            Some(state) => Ok(DevicePacket { number: pads.1, state, subtype: 1 }),
            None => { cache.invalidate(); Err(DeviceError::Disconnected) }
        }
    }

    const BUTTONS: [(u16, KeyCode, GamepadButton); 14] = [
        (0x0001, KeyCode::Digit1, GamepadButton::DPadUp),
        (0x0002, KeyCode::Digit2, GamepadButton::DPadDown),
        (0x0004, KeyCode::Digit3, GamepadButton::DPadLeft),
        (0x0008, KeyCode::Digit4, GamepadButton::DPadRight),
        (0x0010, KeyCode::Enter, GamepadButton::Start),
        (0x0020, KeyCode::Backspace, GamepadButton::Select),
        (0x0040, KeyCode::KeyV, GamepadButton::LeftThumb),
        (0x0080, KeyCode::KeyB, GamepadButton::RightThumb),
        (0x0100, KeyCode::KeyQ, GamepadButton::LeftTrigger),
        (0x0200, KeyCode::KeyR, GamepadButton::RightTrigger),
        (0x1000, KeyCode::Space, GamepadButton::South),
        (0x2000, KeyCode::KeyE, GamepadButton::East),
        (0x4000, KeyCode::ShiftLeft, GamepadButton::West),
        (0x8000, KeyCode::KeyF, GamepadButton::North),
    ];
    fn axis(v: f32) -> i16 { (v.clamp(-1., 1.) * 32767.) as i16 }
    fn trigger(v: f32) -> u8 { (v.clamp(0., 1.) * 255.) as u8 }
    fn keys_axis(keys: &ButtonInput<KeyCode>, neg: KeyCode, pos: KeyCode) -> f32 {
        f32::from(i8::from(keys.pressed(pos)) - i8::from(keys.pressed(neg)))
    }
    fn gamepad_state(pad: &Gamepad) -> XboxState {
        let mut buttons = 0;
        for (bit, _, button) in BUTTONS { if pad.pressed(button) { buttons |= bit; } }
        let get = |a: GamepadAxis| pad.get(a).unwrap_or(0.);
        let analog = |b: GamepadButton| pad.get(b).unwrap_or(if pad.pressed(b) { 1. } else { 0. });
        XboxState {
            buttons,
            triggers: [trigger(analog(GamepadButton::LeftTrigger2)), trigger(analog(GamepadButton::RightTrigger2))],
            left: [axis(get(GamepadAxis::LeftStickX)), axis(get(GamepadAxis::LeftStickY))],
            right: [axis(get(GamepadAxis::RightStickX)), axis(get(GamepadAxis::RightStickY))],
        }
    }
    /// Keyboard layout (XInput equivalents): WASD left stick, arrows right
    /// stick (flicks), Space A, E B, Left Shift X, F Y, Z/C LT/RT, Q/R LB/RB,
    /// Enter Start (also Escape), Backspace Back, 1-4 D-pad, V/B stick clicks.
    fn keyboard_state(keys: &ButtonInput<KeyCode>) -> XboxState {
        let mut buttons = 0;
        for (bit, key, _) in BUTTONS { if keys.pressed(key) { buttons |= bit; } }
        if keys.pressed(KeyCode::Escape) { buttons |= 0x0010; }
        let pull = |k: KeyCode| if keys.pressed(k) { 255 } else { 0 };
        XboxState {
            buttons,
            triggers: [pull(KeyCode::KeyZ), pull(KeyCode::KeyC)],
            left: [axis(keys_axis(keys, KeyCode::KeyA, KeyCode::KeyD)), axis(keys_axis(keys, KeyCode::KeyS, KeyCode::KeyW))],
            right: [axis(keys_axis(keys, KeyCode::ArrowLeft, KeyCode::ArrowRight)), axis(keys_axis(keys, KeyCode::ArrowDown, KeyCode::ArrowUp))],
        }
    }
    fn merge(a: XboxState, b: XboxState) -> XboxState {
        let pick = |x: i16, y: i16| if x.unsigned_abs() >= y.unsigned_abs() { x } else { y };
        XboxState {
            buttons: a.buttons | b.buttons,
            triggers: [a.triggers[0].max(b.triggers[0]), a.triggers[1].max(b.triggers[1])],
            left: [pick(a.left[0], b.left[0]), pick(a.left[1], b.left[1])],
            right: [pick(a.right[0], b.right[0]), pick(a.right[1], b.right[1])],
        }
    }
    /// On-screen touch pad (`web/touch.js`): `window.skateTouch` is
    /// `[buttons, lt, rt, lx, ly, rx, ry]` with triggers 0..1 and axes -1..1.
    fn touch_state() -> Option<XboxState> {
        let value = js_sys::Reflect::get(&js_sys::global(), &"skateTouch".into()).ok()?;
        let array = value.dyn_into::<js_sys::Array>().ok()?;
        let n = |i: u32| array.get(i).as_f64().unwrap_or(0.) as f32;
        Some(XboxState {
            buttons: n(0) as u16,
            triggers: [trigger(n(1)), trigger(n(2))],
            left: [axis(n(3)), axis(n(4))],
            right: [axis(n(5)), axis(n(6))],
        })
    }
    fn mouse_stick() -> Option<[i16; 2]> {
        let value = js_sys::Reflect::get(&js_sys::global(), &"skateMouseStick".into()).ok()?;
        let values = value.dyn_into::<js_sys::Array>().ok()?;
        if values.length() < 2 { return None; }
        let x = values.get(0).as_f64()? as f32;
        let y = values.get(1).as_f64()? as f32;
        Some([axis(x), axis(y)])
    }
    /// PreUpdate, before `input::poll_controllers`.
    pub(crate) fn sample(keys: Option<Res<ButtonInput<KeyCode>>>, gamepads: Query<&Gamepad>) {
        let mut states = [None; 4];
        let mut pads = gamepads.iter().map(gamepad_state);
        let mut keyboard = keys.map(|k| keyboard_state(&k)).unwrap_or_default();
        if let Some(stick) = mouse_stick() {
            if stick != [0, 0] { keyboard.right = stick; }
        }
        let keyboard = match touch_state() { Some(touch) => merge(keyboard, touch), None => keyboard };
        states[0] = Some(match pads.next() { Some(pad) => merge(keyboard, pad), None => keyboard });
        for (slot, pad) in states[1..].iter_mut().zip(pads) { *slot = Some(pad); }
        let mut shared = PADS.lock().unwrap();
        shared.0 = states;
        shared.1 = shared.1.wrapping_add(1);
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    #[test]
    fn capability_cache_refreshes_and_never_caches_errors() {
        let start = skate_vfs::time::Instant::now();
        let mut cache = CapabilityCache::default();
        assert_eq!(cache.get(start, || Ok(1)), Ok(1));
        assert_eq!(
            cache.get(start + skate_vfs::time::Duration::from_millis(999), || panic!(
                "redundant capability query"
            )),
            Ok(1)
        );
        assert_eq!(
            cache.get(start + skate_vfs::time::Duration::from_secs(1), || Ok(2)),
            Ok(2)
        );
        cache.invalidate();
        assert_eq!(
            cache.get(start, || Err(DeviceError::Capabilities(5))),
            Err(DeviceError::Capabilities(5))
        );
        assert_eq!(cache.get(start, || Ok(3)), Ok(3));
        cache.invalidate();
        assert_eq!(cache.get(start, || Ok(4)), Ok(4));
    }
}

// Preserve the uncached API for menu-only polling.
pub(crate) fn poll(index: usize) -> Result<DevicePacket, DeviceError> {
    poll_cached(index, &mut CapabilityCache::default())
}
