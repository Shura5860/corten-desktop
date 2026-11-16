use std::ffi::CString;
use std::fs;
use std::mem::MaybeUninit;

#[derive(Debug, Clone)]
pub struct DiskPartitionInfo {
    #[allow(dead_code)]
    pub mount: String,
    pub total_gb: f32,
    pub used_gb: f32,
    pub used_pct: u8,
}

#[derive(Debug, Clone, Default)]
pub struct DiskMetrics {
    pub root: Option<DiskPartitionInfo>,
    pub home: Option<DiskPartitionInfo>,
    #[allow(dead_code)]
    pub io_active: bool,
}

pub struct DiskMonitor {
    prev_sectors_read: u64,
    prev_sectors_written: u64,
}

impl DiskMonitor {
    pub fn new() -> Self {
        Self {
            prev_sectors_read: 0,
            prev_sectors_written: 0,
        }
    }

    pub fn sample(&mut self) -> DiskMetrics {
        let root = Self::get_mount_info("/");
        let home = Self::get_mount_info("/home");

        let (sectors_read, sectors_written) = Self::read_diskstats();
        let io_active = if self.prev_sectors_read > 0 {
            (sectors_read > self.prev_sectors_read) || (sectors_written > self.prev_sectors_written)
        } else {
            false
        };

        self.prev_sectors_read = sectors_read;
        self.prev_sectors_written = sectors_written;

        DiskMetrics {
            root,
            home,
            io_active,
        }
    }

    fn get_mount_info(path: &str) -> Option<DiskPartitionInfo> {
        let c_path = CString::new(path).ok()?;
        let mut stat = MaybeUninit::<libc::statvfs>::uninit();

        unsafe {
            if libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) == 0 {
                let s = stat.assume_init();
                let block_size = s.f_frsize as f64;
                let total_bytes = s.f_blocks as f64 * block_size;
                let free_bytes = s.f_bavail as f64 * block_size;
                let used_bytes = (total_bytes - free_bytes).max(0.0);

                let total_gb = (total_bytes / (1024.0 * 1024.0 * 1024.0)) as f32;
                let used_gb = (used_bytes / (1024.0 * 1024.0 * 1024.0)) as f32;
                let used_pct = if total_gb > 0.0 {
                    ((used_gb / total_gb) * 100.0).min(100.0) as u8
                } else {
                    0
                };

                return Some(DiskPartitionInfo {
                    mount: path.to_string(),
                    total_gb,
                    used_gb,
                    used_pct,
                });
            }
        }

        None
    }

    fn read_diskstats() -> (u64, u64) {
        if let Ok(content) = fs::read_to_string("/proc/diskstats") {
            let mut total_read = 0u64;
            let mut total_written = 0u64;

            for line in content.lines() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 10 {
                    let dev = parts[2];
                    if dev.starts_with("sd") || dev.starts_with("nvme") || dev.starts_with("vd") {
                        let read: u64 = parts[5].parse().unwrap_or(0);
                        let written: u64 = parts[9].parse().unwrap_or(0);
                        total_read += read;
                        total_written += written;
                    }
                }
            }

            return (total_read, total_written);
        }

        (0, 0)
    }
}
