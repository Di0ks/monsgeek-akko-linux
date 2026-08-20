// Shared types used across multiple TUI tabs

use std::path::PathBuf;

use tokio::sync::mpsc;

use crate::TriggerSettings;
use crate::device_loader::PollingRateSupport;
use crate::firmware_api::FirmwareCheckResult;
use crate::hid::BatteryInfo;
use crate::keymap::KeyRow;
use monsgeek_keyboard::TravelDepth;
use monsgeek_keyboard::{
    KeyboardOptions as KbOptions, LedParams, Precision, RT_STABILITY_MAX, SleepTimeSettings,
};
use monsgeek_transport::TransportType;
use monsgeek_transport::protocol::Profile;

#[cfg(feature = "notify")]
use crate::effect::EffectLibrary;

/// Map TransportType to a short display name
pub(crate) fn transport_type_name(tt: TransportType) -> &'static str {
    match tt {
        TransportType::HidWired => "usb",
        TransportType::HidDongle => "dongle",
        TransportType::Bluetooth => "bt",
        TransportType::WebRtc => "webrtc",
    }
}

/// Look up what polling rate control, if any, a connected device offers.
///
/// Returns the support requirement plus the rates it accepts, fastest first. Unknown
/// devices get no control rather than the full rate list, since writing a rate the
/// firmware does not implement has no defined behaviour.
pub(crate) fn resolve_polling_rate(
    device_id: Option<i32>,
    vid: u16,
    pid: u16,
    transport: TransportType,
) -> (PollingRateSupport, &'static [u16]) {
    let registry = crate::profile_registry();
    let Some(def) = device_id
        .and_then(|id| registry.get_device_info_by_id_and_usb(id, vid, pid))
        .or_else(|| registry.get_device_info(vid, pid))
    else {
        return (PollingRateSupport::Unsupported, &[]);
    };
    let over_bluetooth = transport == TransportType::Bluetooth;
    (
        def.polling_rate_support(over_bluetooth),
        def.polling_rates(),
    )
}

/// Battery data source
#[derive(Debug, Clone)]
pub(crate) enum BatterySource {
    /// Kernel power_supply sysfs (via eBPF filter)
    Kernel(PathBuf),
    /// Direct vendor protocol (HID feature report)
    Vendor,
}

/// Parsed patch info for display
#[derive(Debug, Clone)]
pub(crate) struct PatchInfoData {
    /// Patch name (e.g. "MONSMOD")
    pub name: String,
    /// Patch version
    pub version: u8,
    /// Capability names (e.g. ["battery", "led_stream"])
    pub capabilities: Vec<&'static str>,
}

/// Keyboard options state
#[derive(Debug, Clone, Default)]
pub(crate) struct KeyboardOptions {
    pub os_mode: u8,
    pub fn_layer: u8,
    pub anti_mistouch: bool,
    pub rt_stability: u8,
    pub wasd_swap: bool,
    // Sleep time settings (all in seconds, 0 = disabled)
    pub idle_bt: u16,
    pub idle_24g: u16,
    pub deep_bt: u16,
    pub deep_24g: u16,
}

/// Sleep time field identifier for updates
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum SleepField {
    IdleBt,
    Idle24g,
    DeepBt,
    Deep24g,
}

/// Key depth visualization mode
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) enum DepthViewMode {
    #[default]
    BarChart, // Bar chart of all active keys
    TimeSeries, // Time series graph of selected keys
}

/// How a spinner renders its integer value.
#[derive(Debug, Clone, Copy)]
pub(crate) enum SpinnerDisplay {
    /// The number itself, with a unit suffix ("ms", "s", or "" for bare counts).
    Integer(&'static str),
    /// Raw travel units, rendered as millimetres through the device precision.
    Travel(Precision),
}

/// A spinner over an **integer** value, in whatever unit the value is stored in.
///
/// Every spinner field is an integer underneath — raw travel units, milliseconds,
/// an RGB component. The previous version stepped in `f32` millimetres and
/// truncated on save, so a value could not survive being opened and closed: five
/// `+0.05mm` steps from raw 204 landed on 228 rather than 229. Stepping in the
/// stored unit makes increments exact and the round trip lossless; mm bounds are
/// converted to raw units once, at construction.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SpinnerConfig {
    pub min: u16,
    pub max: u16,
    pub step: u16,
    pub step_coarse: u16,
    pub display: SpinnerDisplay,
}

impl SpinnerConfig {
    /// A spinner over a bare integer (counts, milliseconds, seconds).
    pub const fn integer(
        min: u16,
        max: u16,
        step: u16,
        step_coarse: u16,
        unit: &'static str,
    ) -> Self {
        Self {
            min,
            max,
            step,
            step_coarse,
            display: SpinnerDisplay::Integer(unit),
        }
    }

    /// A travel spinner whose bounds are given in millimetres. The conversion to
    /// raw units happens here, once — stepping afterwards is integer arithmetic.
    pub fn travel_mm(
        min_mm: f64,
        max_mm: f64,
        step_mm: f64,
        step_coarse_mm: f64,
        precision: Precision,
    ) -> Self {
        Self {
            min: precision.mm_to_raw(min_mm),
            max: precision.mm_to_raw(max_mm),
            // A step must move at least one raw unit, or the spinner sticks.
            step: precision.mm_to_raw(step_mm).max(1),
            step_coarse: precision.mm_to_raw(step_coarse_mm).max(1),
            display: SpinnerDisplay::Travel(precision),
        }
    }

    /// Increment by step (or the coarse step if shift is held).
    pub fn increment(&self, value: u16, coarse: bool) -> u16 {
        let step = if coarse { self.step_coarse } else { self.step };
        value.saturating_add(step).clamp(self.min, self.max)
    }

    /// Decrement by step (or the coarse step if shift is held).
    pub fn decrement(&self, value: u16, coarse: bool) -> u16 {
        let step = if coarse { self.step_coarse } else { self.step };
        value.saturating_sub(step).clamp(self.min, self.max)
    }

    /// Increment a `u8`-valued setting (RGB components, brightness, profile).
    pub fn increment_u8(&self, value: u8, coarse: bool) -> u8 {
        self.increment(value.into(), coarse).min(u8::MAX.into()) as u8
    }

    /// Decrement a `u8`-valued setting.
    pub fn decrement_u8(&self, value: u8, coarse: bool) -> u8 {
        self.decrement(value.into(), coarse).min(u8::MAX.into()) as u8
    }

    /// Render the value the way this spinner displays it, without the unit.
    pub fn format(&self, value: u16) -> String {
        match self.display {
            SpinnerDisplay::Integer(_) => value.to_string(),
            SpinnerDisplay::Travel(p) => TravelDepth::from_raw(value).format_mm(p),
        }
    }

    /// Unit suffix for display.
    pub fn unit(&self) -> &'static str {
        match self.display {
            SpinnerDisplay::Integer(u) => u,
            SpinnerDisplay::Travel(_) => "mm",
        }
    }
}

/// Spinner config for RGB color components (0-255)
pub(crate) const RGB_SPINNER: SpinnerConfig = SpinnerConfig::integer(0, 255, 1, 10, "");

/// Spinner config for LED brightness (0-4)
pub(crate) const BRIGHTNESS_SPINNER: SpinnerConfig = SpinnerConfig::integer(0, 4, 1, 1, "");

/// Spinner config for LED speed (0-4)
pub(crate) const SPEED_SPINNER: SpinnerConfig = SpinnerConfig::integer(0, 4, 1, 1, "");

/// Spinner config for debounce (0-25, step 1, coarse 5)
pub(crate) const DEBOUNCE_SPINNER: SpinnerConfig = SpinnerConfig::integer(0, 25, 1, 5, "");

/// Spinner config for Fn layer (0-1; the firmware keeps a single bit)
pub(crate) const FN_LAYER_SPINNER: SpinnerConfig = SpinnerConfig::integer(0, 1, 1, 1, "");

/// Spinner config for RT stability level (0-5, 25 ms each)
pub(crate) const RT_STABILITY_SPINNER: SpinnerConfig =
    SpinnerConfig::integer(0, RT_STABILITY_MAX as u16, 1, 1, "");

/// Spinner config for OS mode (0=Windows, 1=macOS, 2=iOS, 3=Android)
pub(crate) const OS_MODE_SPINNER: SpinnerConfig = SpinnerConfig::integer(0, 3, 1, 1, "");

/// Spinner config for sleep time in seconds (0-3600, step 60s, coarse 300s)
pub(crate) const SLEEP_TIME_SPINNER: SpinnerConfig = SpinnerConfig::integer(0, 3600, 60, 300, "s");

/// Loading state for async data fetching
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) enum LoadState {
    #[default]
    NotLoaded,
    Loading,
    Loaded,
    Error,
}

/// Track loading state per HID query group
#[derive(Debug, Clone, Default)]
pub(crate) struct LoadingStates {
    // Device info queries (tab 0/1)
    pub usb_version: LoadState, // device_id + version
    pub profile: LoadState,
    pub debounce: LoadState,
    pub polling_rate: LoadState,
    pub led_params: LoadState, // all main LED fields
    pub side_led_params: LoadState,
    pub kb_options_info: LoadState, // fn_layer + wasd_swap for info display
    pub precision: LoadState,
    pub sleep_time: LoadState,
    pub patch_info: LoadState,
    pub dongle_patch_info: LoadState,
    pub firmware_check: LoadState, // server firmware version check
    // Other tabs
    pub triggers: LoadState,    // tab 3
    pub options: LoadState,     // tab 4
    pub key_mapping: LoadState, // unified Key Mapping tab
}

/// Async result from background keyboard operations
/// These are sent from spawned tasks to the main event loop
#[allow(dead_code)] // Macros and SetComplete reserved for future use
pub(crate) enum AsyncResult {
    // Device info results
    DeviceIdAndVersion(Result<(u32, monsgeek_keyboard::FirmwareVersion), String>),
    Profile(Result<Profile, String>),
    Debounce(Result<u8, String>),
    PollingRate(Result<u16, String>),
    LedParams(Result<LedParams, String>),
    SideLedParams(Result<LedParams, String>),
    KbOptions(Result<KbOptions, String>),
    Precision(Result<Precision, String>),
    SleepTime(Result<SleepTimeSettings, String>),
    PatchInfo(Result<PatchInfoData, String>),
    DonglePatchInfo(Result<PatchInfoData, String>),
    FirmwareCheck(FirmwareCheckResult),
    // Other tab results
    Triggers(Result<TriggerSettings, String>),
    Options(Result<KbOptions, String>),
    KeyRows(Result<Vec<KeyRow>, String>),
    /// Which table the key-mapping load is on, for the progress line
    KeyRowsStage(crate::keymap::LoadStage),
    // Battery status (from keyboard API)
    Battery(Result<BatteryInfo, String>),
    // Operation completion (for set operations)
    SetComplete(String, Result<(), String>), // (field_name, result)
    // Notify tab
    #[cfg(feature = "notify")]
    NotifyEffectsLoaded(Result<EffectLibrary, String>),
    #[cfg(feature = "notify")]
    NotifyDaemonStopped(Result<(), String>),
    #[cfg(feature = "notify")]
    NotifyList(Vec<(u64, String, String, String, i32)>),
    // Animation engine status
    AnimStatus(Result<crate::anim::EngineSnapshot, String>),
}

/// An async result tagged with the device generation it was produced for.
pub(crate) struct GenerationalResult {
    pub generation: u64,
    pub result: AsyncResult,
}

/// A sender that automatically tags results with a device generation.
#[derive(Clone)]
pub(crate) struct GenSender {
    pub tx: mpsc::UnboundedSender<GenerationalResult>,
    pub generation: u64,
}

impl GenSender {
    pub fn send(&self, result: AsyncResult) {
        let _ = self.tx.send(GenerationalResult {
            generation: self.generation,
            result,
        });
    }
}

/// History length for time series (samples)
pub(crate) const DEPTH_HISTORY_LEN: usize = 100;

/// The spinner steps in the unit the value is *stored* in, so an edit is exact.
/// The previous `f32`-millimetre spinner truncated on save and could not survive
/// a round trip — these pin that it now does.
#[cfg(test)]
mod spinner_tests {
    use super::*;
    use monsgeek_keyboard::Precision;

    /// Stepping up N times and back down N times must return the original raw
    /// value. Under the old `f32` path, five `+0.05mm` steps from raw 204 landed
    /// on 228 rather than 229, and the error survived into flash.
    #[test]
    fn travel_edits_are_lossless() {
        for precision in [Precision::Coarse, Precision::Medium, Precision::Fine] {
            let cfg = SpinnerConfig::travel_mm(0.1, 4.0, 0.05, 0.2, precision);
            for start in [cfg.min, 150, 204, 250, cfg.max / 2] {
                if start < cfg.min || start > cfg.max {
                    continue;
                }
                for coarse in [false, true] {
                    for steps in 1..=5 {
                        let mut v = start;
                        for _ in 0..steps {
                            v = cfg.increment(v, coarse);
                        }
                        for _ in 0..steps {
                            v = cfg.decrement(v, coarse);
                        }
                        assert_eq!(
                            v, start,
                            "{precision:?}: {steps}x +/- (coarse={coarse}) from {start} drifted"
                        );
                    }
                }
            }
        }
    }

    /// A step must move at least one raw unit, or the field appears frozen —
    /// 0.05mm rounds to 0 raw units at 0.1mm precision.
    #[test]
    fn a_step_always_moves_the_value() {
        for precision in [Precision::Coarse, Precision::Medium, Precision::Fine] {
            let cfg = SpinnerConfig::travel_mm(0.1, 4.0, 0.05, 0.2, precision);
            assert!(cfg.step >= 1, "{precision:?}: fine step rounds to nothing");
            assert!(
                cfg.step_coarse >= 1,
                "{precision:?}: coarse step rounds to nothing"
            );
            let mid = (cfg.min + cfg.max) / 2;
            assert!(cfg.increment(mid, false) > mid);
            assert!(cfg.decrement(mid, false) < mid);
        }
    }

    #[test]
    fn stepping_saturates_at_the_bounds_without_wrapping() {
        let cfg = SpinnerConfig::integer(10, 25, 5, 100, "");
        assert_eq!(cfg.increment(24, false), 25);
        assert_eq!(cfg.increment(25, true), 25);
        assert_eq!(cfg.decrement(11, false), 10);
        // A coarse step wider than the whole range must clamp, not wrap.
        assert_eq!(cfg.decrement(12, true), 10);
    }

    #[test]
    fn travel_bounds_come_from_the_mm_values() {
        // 0.1..4.0mm at 0.01mm/unit is 10..400 raw; the 0.05mm step is 5 units.
        let cfg = SpinnerConfig::travel_mm(0.1, 4.0, 0.05, 0.2, Precision::Medium);
        assert_eq!(
            (cfg.min, cfg.max, cfg.step, cfg.step_coarse),
            (10, 400, 5, 20)
        );
        assert_eq!(cfg.format(204), "2.04");
        assert_eq!(cfg.unit(), "mm");
    }
}
