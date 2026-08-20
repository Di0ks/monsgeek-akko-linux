//! Keyboard settings types

/// Precision level for trigger/travel settings
///
/// Determines the resolution of travel distance measurements.
/// Higher precision allows finer control over actuation points.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Precision {
    /// 0.1mm resolution (legacy/low precision)
    #[default]
    Coarse,
    /// 0.01mm resolution (standard precision)
    Medium,
    /// 0.005mm resolution (high precision)
    Fine,
}

impl Precision {
    /// Create from feature list precision byte
    ///
    /// The feature list response uses: 0 = 0.1mm, 1 = 0.05mm, 2 = 0.01mm
    /// Note: 0.05mm maps to Medium since we don't have a separate variant
    pub fn from_feature_byte(byte: u8) -> Self {
        match byte {
            2 => Self::Fine,   // 0.005mm - highest precision
            1 => Self::Medium, // 0.01mm (0.05mm in feature list)
            _ => Self::Coarse, // 0.1mm - default/legacy
        }
    }

    /// Create from firmware version
    ///
    /// Older firmware doesn't support feature list, so precision is
    /// inferred from version number thresholds.
    pub fn from_firmware_version(version: u16) -> Self {
        use monsgeek_transport::protocol::precision;
        if version >= precision::FINE_VERSION {
            Self::Fine // 0.005mm
        } else if version >= precision::MEDIUM_VERSION {
            Self::Medium // 0.01mm
        } else {
            Self::Coarse // 0.1mm
        }
    }

    /// Get the precision factor (multiplier for raw values)
    ///
    /// Raw travel values are multiplied by 1/factor to get mm.
    /// E.g., raw value 100 with factor 100 = 1.0mm
    pub fn factor(&self) -> f64 {
        match self {
            Self::Fine => 200.0,   // 0.005mm steps
            Self::Medium => 100.0, // 0.01mm steps
            Self::Coarse => 10.0,  // 0.1mm steps
        }
    }

    /// Get precision as display string
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Fine => "0.005mm",
            Self::Medium => "0.01mm",
            Self::Coarse => "0.1mm",
        }
    }

    /// Convert raw travel value to millimeters
    pub fn raw_to_mm(&self, raw: u16) -> f64 {
        raw as f64 / self.factor()
    }

    /// Convert millimeters to raw travel value
    pub fn mm_to_raw(&self, mm: f64) -> u16 {
        (mm * self.factor()).round() as u16
    }

    /// Decimal places needed to render one raw unit *exactly*.
    ///
    /// A device with 0.005mm units cannot be shown faithfully in two decimals,
    /// so the display follows the device rather than a fixed width. Rendering at
    /// this many places means no rounding happens on the way to the screen.
    pub const fn decimals(&self) -> u32 {
        match self {
            Self::Coarse => 1, // 0.1mm
            Self::Medium => 2, // 0.01mm
            Self::Fine => 3,   // 0.005mm
        }
    }

    /// Raw travel in micrometres — exact integer arithmetic, since 1000µm is a
    /// whole multiple of every unit size (100 / 10 / 5 µm).
    pub const fn raw_to_um(&self, raw: u16) -> u32 {
        raw as u32
            * match self {
                Self::Coarse => 100,
                Self::Medium => 10,
                Self::Fine => 5,
            }
    }
}

/// Firmware version information
#[derive(Debug, Clone, Default)]
pub struct FirmwareVersion {
    /// Packed version word: major in the high byte, minor in the low byte.
    /// e.g. 0x0408 = v408. The 0x8F handler sends minor first, then major.
    pub raw: u16,
}

impl FirmwareVersion {
    /// Create from raw version number
    pub fn new(raw: u16) -> Self {
        Self { raw }
    }

    /// Parse version from GET_REV response bytes (starting after cmd echo)
    /// Format: bytes 0-3 = device_id, bytes 7-8 = version (little-endian u16)
    pub fn from_bytes(bytes: &[u8]) -> Self {
        if bytes.len() < 9 {
            return Self::default();
        }
        let raw = u16::from_le_bytes([bytes[7], bytes[8]]);
        Self { raw }
    }

    /// Get precision level based on firmware version
    pub fn precision(&self) -> Precision {
        Precision::from_firmware_version(self.raw)
    }

    /// Get precision factor based on firmware version
    /// Newer firmware has higher precision for travel settings
    pub fn precision_factor(&self) -> f64 {
        self.precision().factor()
    }

    /// Get precision string (e.g., "0.01mm")
    pub fn precision_str(&self) -> &'static str {
        self.precision().as_str()
    }

    /// Format the way the vendor names firmware: `"v408"` for raw `0x0408`.
    ///
    /// The word is packed major/minor — high byte 4, low byte 8 — and the two
    /// are written together without a separator, matching the release archives
    /// (`..._KB_V407_...`, `..._KB_V408`) and the images in `firmwares/`
    /// (`v300`, `v316`, `v405`, `v407`, `v408`).
    ///
    /// Reading the word as a decimal number instead renders `0x0408` as
    /// "v10.32", which is how this used to print.
    pub fn format(&self) -> String {
        format!("v{}{:02}", self.raw >> 8, self.raw & 0xFF)
    }

    /// Format as major.minor.patch (e.g., "4.0.5" for raw=0x405)
    pub fn format_dotted(&self) -> String {
        let major = (self.raw >> 8) & 0xF;
        let minor = (self.raw >> 4) & 0xF;
        let patch = self.raw & 0xF;
        format!("{major}.{minor}.{patch}")
    }

    /// Get precision factor from raw version number (static)
    pub fn precision_factor_from_raw(version: u16) -> f32 {
        use monsgeek_transport::protocol::precision;
        if version >= precision::FINE_VERSION {
            200.0 // 0.005mm precision
        } else if version >= precision::MEDIUM_VERSION {
            100.0 // 0.01mm precision
        } else {
            10.0 // 0.1mm precision
        }
    }

    /// Decode precision byte from feature list response
    /// Returns human-readable precision string
    pub fn precision_byte_str(precision: u8) -> &'static str {
        match precision {
            0 => "0.1mm",
            1 => "0.05mm",
            2 => "0.01mm",
            _ => "unknown",
        }
    }
}

/// Battery information (wireless only)
#[derive(Debug, Clone, Default)]
pub struct BatteryInfo {
    /// Battery level 0-100
    pub level: u8,
    /// Device is online/connected
    pub online: bool,
    /// Device is charging (may not be available)
    pub charging: bool,
    /// Device is idle (no recent key activity)
    pub idle: bool,
}

pub use monsgeek_transport::command::PollingRate;

/// Keyboard options (`SET_KBOPTION` 0x09 / `GET_KBOPTION` 0x89)
///
/// Payload layout, firmware-validated against v407 (`case 9` and `case 0x89` of the
/// vendor command dispatch) and the vendor RY5088 web driver:
/// `[cmd, os_mode, fn_layer, anti_mistouch, rt_stability, wasd_swap]`.
///
/// Both commands carry the whole set, so changing one field means read-modify-write.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyboardOptions {
    /// OS mode (0=Windows, 1=macOS, 2=iOS, 3=Android); firmware keeps 2 bits
    pub os_mode: u8,
    /// Fn layer index; firmware keeps 1 bit, so only 0 or 1
    pub fn_layer: u8,
    /// Anti-mistouch enabled
    pub anti_mistouch: bool,
    /// Rapid Trigger stability level (0=off, 1-5), [`RT_STABILITY_STEP_MS`] per level
    pub rt_stability: u8,
    /// WASD/arrow swap (the Fn+W toggle)
    pub wasd_swap: bool,
}

/// Milliseconds per Rapid Trigger stability level
pub const RT_STABILITY_STEP_MS: u16 = 25;

/// Highest Rapid Trigger stability level the firmware accepts
pub const RT_STABILITY_MAX: u8 = 5;

impl KeyboardOptions {
    /// Parse from GET_KBOPTION response payload (response bytes after the command byte)
    pub fn from_bytes(bytes: &[u8]) -> Self {
        if bytes.len() < 5 {
            return Self::default();
        }
        Self {
            os_mode: bytes[0] & 0x03,
            fn_layer: bytes[1] & 0x01,
            anti_mistouch: bytes[2] != 0,
            // The firmware happily stores out-of-range levels but treats them as off
            rt_stability: if bytes[3] > RT_STABILITY_MAX {
                0
            } else {
                bytes[3]
            },
            wasd_swap: bytes[4] != 0,
        }
    }

    /// Convert to protocol bytes for SET_KBOPTION
    pub fn to_bytes(&self) -> [u8; 5] {
        [
            self.os_mode & 0x03,
            self.fn_layer & 0x01,
            u8::from(self.anti_mistouch),
            self.rt_stability.min(RT_STABILITY_MAX),
            u8::from(self.wasd_swap),
        ]
    }

    /// Rapid Trigger stability window in milliseconds
    pub fn rt_stability_ms(&self) -> u16 {
        self.rt_stability as u16 * RT_STABILITY_STEP_MS
    }

    /// Human-readable OS mode
    pub fn os_mode_name(&self) -> &'static str {
        os_mode_name(self.os_mode)
    }
}

/// Human-readable name for an OS mode value
pub fn os_mode_name(mode: u8) -> &'static str {
    match mode {
        0 => "Windows",
        1 => "macOS",
        2 => "iOS",
        3 => "Android",
        _ => "Unknown",
    }
}

/// Sleep time settings for wireless modes
///
/// Controls idle and deep sleep timeouts for Bluetooth and 2.4GHz connections.
/// Times are in seconds. Set to 0 to disable that particular timeout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SleepTimeSettings {
    /// Bluetooth idle timeout (seconds) - keyboard enters light sleep
    pub idle_bt: u16,
    /// 2.4GHz idle timeout (seconds) - keyboard enters light sleep
    pub idle_24g: u16,
    /// Bluetooth deep sleep timeout (seconds) - keyboard powers down further
    pub deep_bt: u16,
    /// 2.4GHz deep sleep timeout (seconds) - keyboard powers down further
    pub deep_24g: u16,
}

impl Default for SleepTimeSettings {
    fn default() -> Self {
        Self {
            idle_bt: 120,   // 2 minutes
            idle_24g: 120,  // 2 minutes
            deep_bt: 1680,  // 28 minutes
            deep_24g: 1680, // 28 minutes
        }
    }
}

impl SleepTimeSettings {
    /// Create new sleep time settings
    pub fn new(idle_bt: u16, idle_24g: u16, deep_bt: u16, deep_24g: u16) -> Self {
        Self {
            idle_bt,
            idle_24g,
            deep_bt,
            deep_24g,
        }
    }

    /// Create with same idle and deep timeout for both wireless modes
    pub fn uniform(idle_seconds: u16, deep_seconds: u16) -> Self {
        Self {
            idle_bt: idle_seconds,
            idle_24g: idle_seconds,
            deep_bt: deep_seconds,
            deep_24g: deep_seconds,
        }
    }

    /// Format idle timeout as human-readable duration
    pub fn format_idle(&self, is_bt: bool) -> String {
        let secs = if is_bt { self.idle_bt } else { self.idle_24g };
        Self::format_duration(secs)
    }

    /// Format deep sleep timeout as human-readable duration
    pub fn format_deep(&self, is_bt: bool) -> String {
        let secs = if is_bt { self.deep_bt } else { self.deep_24g };
        Self::format_duration(secs)
    }

    /// Format seconds as human-readable duration string
    pub fn format_duration(secs: u16) -> String {
        if secs == 0 {
            "disabled".to_string()
        } else if secs < 60 {
            format!("{}s", secs)
        } else if secs < 3600 {
            let mins = secs / 60;
            let rem = secs % 60;
            if rem == 0 {
                format!("{}m", mins)
            } else {
                format!("{}m {}s", mins, rem)
            }
        } else {
            let hours = secs / 3600;
            let mins = (secs % 3600) / 60;
            if mins == 0 {
                format!("{}h", hours)
            } else {
                format!("{}h {}m", hours, mins)
            }
        }
    }

    /// Parse duration string (e.g., "2m", "30s", "1h 30m") to seconds
    pub fn parse_duration(s: &str) -> Option<u16> {
        let s = s.trim().to_lowercase();

        // Handle "disabled" or "off"
        if s == "disabled" || s == "off" || s == "0" {
            return Some(0);
        }

        let mut total_secs: u32 = 0;
        let mut current_num = String::new();

        for c in s.chars() {
            if c.is_ascii_digit() {
                current_num.push(c);
            } else if !current_num.is_empty() {
                let num: u32 = current_num.parse().ok()?;
                current_num.clear();
                match c {
                    'h' => total_secs += num * 3600,
                    'm' => total_secs += num * 60,
                    's' => total_secs += num,
                    _ => return None,
                }
            }
        }

        // If there's a trailing number with no unit, treat as seconds
        if !current_num.is_empty() {
            let num: u32 = current_num.parse().ok()?;
            total_secs += num;
        }

        // Clamp to u16 max
        Some(total_secs.min(u16::MAX as u32) as u16)
    }
}

/// Device feature list
#[derive(Debug, Clone, Default)]
pub struct FeatureList {
    /// Precision factor for trigger settings
    pub precision: u8,
    /// Raw feature flags
    pub raw_features: Vec<u8>,
}

impl FeatureList {
    /// Parse from GET_FEATURE_LIST response bytes (after echo byte stripped)
    /// Response format: [0xAA validity marker, precision_enum, ...]
    /// If validity marker is not 0xAA, the response is invalid and precision defaults to 0xFF (unknown)
    pub fn from_bytes(bytes: &[u8]) -> Self {
        // Check validity marker (first byte should be 0xAA)
        let valid = bytes.first().copied() == Some(0xAA);
        Self {
            // Byte 0 = 0xAA validity marker, Byte 1 = precision enum
            // Use 0xFF to indicate unknown/invalid response
            precision: if valid {
                bytes.get(1).copied().unwrap_or(0xFF)
            } else {
                0xFF // Invalid response - will trigger fallback to firmware version
            },
            raw_features: bytes.to_vec(),
        }
    }

    /// Check if the feature list response was valid
    pub fn is_valid(&self) -> bool {
        self.precision != 0xFF
    }

    /// Get precision level from feature list
    ///
    /// Returns None if the feature list response was invalid (command not supported).
    /// Caller should fall back to firmware version in that case.
    pub fn precision(&self) -> Option<Precision> {
        if self.is_valid() {
            Some(Precision::from_feature_byte(self.precision))
        } else {
            None
        }
    }

    /// Get the precision factor (10, 100, or 200)
    pub fn precision_factor(&self) -> f64 {
        self.precision().map(|p| p.factor()).unwrap_or(10.0) // Default to coarse if invalid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Byte 4 is the WASD/arrow swap flag — the same toggle Fn+W drives.
    #[test]
    fn kb_options_wire_layout() {
        // GET_KBOPTION payload: os=macOS, fn=1, anti-mistouch on, RT level 3, swap on
        let opts = KeyboardOptions::from_bytes(&[1, 1, 1, 3, 1]);
        assert_eq!(opts.os_mode, 1);
        assert_eq!(opts.os_mode_name(), "macOS");
        assert_eq!(opts.fn_layer, 1);
        assert!(opts.anti_mistouch);
        assert_eq!(opts.rt_stability, 3);
        assert_eq!(opts.rt_stability_ms(), 75);
        assert!(opts.wasd_swap);

        assert_eq!(opts.to_bytes(), [1, 1, 1, 3, 1]);
    }

    #[test]
    fn kb_options_masks_firmware_field_widths() {
        // The firmware keeps 2 bits of OS mode and 1 bit of Fn layer
        let opts = KeyboardOptions::from_bytes(&[0xFF, 0xFF, 0, 0, 0]);
        assert_eq!(opts.os_mode, 3);
        assert_eq!(opts.fn_layer, 1);

        let clamped = KeyboardOptions {
            os_mode: 7,
            fn_layer: 3,
            rt_stability: 200,
            ..Default::default()
        };
        assert_eq!(clamped.to_bytes(), [3, 1, 0, RT_STABILITY_MAX, 0]);
    }

    /// Out-of-range RT levels read back as off, matching the firmware's own handling
    #[test]
    fn kb_options_rejects_bogus_rt_level() {
        assert_eq!(
            KeyboardOptions::from_bytes(&[0, 0, 0, 100, 0]).rt_stability,
            0
        );
        assert_eq!(KeyboardOptions::from_bytes(&[]), KeyboardOptions::default());
    }
}
