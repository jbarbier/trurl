//! What this computer can do: RAM, physical cores, CPU features.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CpuVariant {
    /// llama.cpp CPU backends that need AVX2 (Haswell, 2013, and newer).
    Avx2,
    /// Backends that run on any x86-64 CPU.
    Compat,
}

impl CpuVariant {
    pub fn dir_name(self) -> &'static str {
        match self {
            CpuVariant::Avx2 => "avx2",
            CpuVariant::Compat => "compat",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Hardware {
    pub total_ram: u64,
    pub physical_cores: usize,
    pub logical_cores: usize,
    pub cpu_name: String,
    pub cpu_features: CpuFeatures,
    pub variant: CpuVariant,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct CpuFeatures {
    pub avx: bool,
    pub avx2: bool,
    pub fma: bool,
    pub f16c: bool,
    pub bmi2: bool,
}

impl CpuFeatures {
    pub fn detect() -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            CpuFeatures {
                avx: std::is_x86_feature_detected!("avx"),
                avx2: std::is_x86_feature_detected!("avx2"),
                fma: std::is_x86_feature_detected!("fma"),
                f16c: std::is_x86_feature_detected!("f16c"),
                bmi2: std::is_x86_feature_detected!("bmi2"),
            }
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            CpuFeatures::default()
        }
    }

    /// llama.cpp's "haswell" CPU backend, the lowest of the AVX2 builds, needs all five.
    pub fn variant(self) -> CpuVariant {
        if self.avx && self.avx2 && self.fma && self.f16c && self.bmi2 { CpuVariant::Avx2 } else { CpuVariant::Compat }
    }
}

impl Hardware {
    pub fn detect() -> Self {
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        sys.refresh_cpu_list(sysinfo::CpuRefreshKind::nothing());
        let logical = sys.cpus().len().max(1);
        let physical = sysinfo::System::physical_core_count().unwrap_or(logical).max(1);
        let cpu_name = sys.cpus().first().map(|c| c.brand().trim().to_string()).unwrap_or_default();
        let features = CpuFeatures::detect();
        Hardware {
            total_ram: sys.total_memory(),
            physical_cores: physical,
            logical_cores: logical,
            cpu_name,
            cpu_features: features,
            variant: features.variant(),
        }
    }

    /// Threads for llama-server: one per physical core. Hyper-threads share the
    /// same execution units and slow token generation down when oversubscribed.
    pub fn threads(&self) -> usize {
        self.physical_cores.max(1)
    }
}

/// RAM still free right now, used for the warning before a load.
pub fn available_ram() -> u64 {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    sys.available_memory()
}

/// The RAM-fit dot shown next to every model (the rule from the spec):
/// green if the file is under 50% of total RAM, orange under 75%, red otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RamFit {
    Green,
    Orange,
    Red,
}

pub fn ram_fit(file_size: u64, total_ram: u64) -> RamFit {
    if total_ram == 0 {
        return RamFit::Red;
    }
    // Integer math: size/ram < 0.5  <=>  2*size < ram.
    let s = file_size as u128;
    let r = total_ram as u128;
    if 2 * s < r {
        RamFit::Green
    } else if 4 * s < 3 * r {
        RamFit::Orange
    } else {
        RamFit::Red
    }
}

/// Free bytes on the volume holding `path` (0 if unknown, which callers treat as "do not block").
pub fn free_space(path: &std::path::Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let c = CString::new(path.as_os_str().as_bytes()).ok()?;
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: c is a valid NUL-terminated path and st is a properly sized out-parameter.
        let rc = unsafe { libc::statvfs(c.as_ptr(), &mut st) };
        if rc != 0 {
            return None;
        }
        Some(st.f_bavail as u64 * st.f_frsize as u64)
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        let mut free: u64 = 0;
        // SAFETY: wide is NUL-terminated; out-pointers are valid or null as the API allows.
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut free,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if ok == 0 { None } else { Some(free) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn ram_fit_thresholds_from_the_spec() {
        // Gemma 3 1B (806 MB) and 4B (2.49 GB) on 4 and 8 GB laptops.
        assert_eq!(ram_fit(806_058_240, 4 * GB), RamFit::Green);
        assert_eq!(ram_fit(2_489_757_856, 4 * GB), RamFit::Orange);
        assert_eq!(ram_fit(2_489_757_856, 8 * GB), RamFit::Green);
        assert_eq!(ram_fit(3 * GB, 4 * GB), RamFit::Red);
        // Exact boundaries: 50% is not "under 50%".
        assert_eq!(ram_fit(2 * GB, 4 * GB), RamFit::Orange);
        assert_eq!(ram_fit(3 * GB, 4 * GB), RamFit::Red);
        assert_eq!(ram_fit(1, 0), RamFit::Red);
    }

    #[test]
    fn variant_needs_all_five_features() {
        let all = CpuFeatures { avx: true, avx2: true, fma: true, f16c: true, bmi2: true };
        assert_eq!(all.variant(), CpuVariant::Avx2);
        for i in 0..5 {
            let mut f = all;
            match i {
                0 => f.avx = false,
                1 => f.avx2 = false,
                2 => f.fma = false,
                3 => f.f16c = false,
                _ => f.bmi2 = false,
            }
            assert_eq!(f.variant(), CpuVariant::Compat, "missing feature {i}");
        }
        assert_eq!(CpuFeatures::default().variant(), CpuVariant::Compat);
    }

    #[test]
    fn detects_something_sane() {
        let h = Hardware::detect();
        assert!(h.total_ram > 256 * 1024 * 1024);
        assert!(h.threads() >= 1 && h.threads() <= h.logical_cores);
        assert!(free_space(std::path::Path::new(".")).unwrap() > 0);
    }
}
