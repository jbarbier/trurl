//! Self-update for the terminal app on Linux.
//!
//! The release tarball is checked against the same minisign key as the desktop
//! updater before anything is touched. The app folder is replaced as a whole:
//! the new version is unpacked next to it, then the two folders are swapped by
//! rename, so a failure halfway leaves the old version working. Models live in
//! the data folder, never in the app folder, so they are not touched.

use std::path::{Path, PathBuf};
use trurl_core::app::App;
use trurl_core::errors::UserError;
use trurl_core::update::{self, UpdateInfo};

fn http() -> reqwest::Client {
    trurl_core::net::client_builder()
        .user_agent(format!("Trurl/{} (terminal)", trurl_core::VERSION))
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .expect("http client")
}

pub async fn check() -> Result<Option<UpdateInfo>, UserError> {
    let url =
        std::env::var("TRURL_UPDATE_URL").unwrap_or_else(|_| update::MANIFEST_URL.to_string());
    update::check(&http(), &url, trurl_core::VERSION).await
}

fn app_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .canonicalize()
        .ok()?
        .parent()
        .map(Path::to_path_buf)
}

/// Marker written by the release tarball and install.sh: this folder holds only
/// Trurl, so replacing it as a whole is safe. Without it (Trurl run from a
/// folder holding other things, like the Desktop) Trurl never touches the folder.
pub const MARKER: &str = ".trurl-app";

/// Linux only, only inside a folder marked as Trurl's own, and only if Trurl may
/// write next to it.
pub fn can_install() -> bool {
    if !cfg!(target_os = "linux") {
        return false;
    }
    let Some(dir) = app_dir() else { return false };
    if !dir.join(MARKER).is_file() {
        return false;
    }
    let Some(parent) = dir.parent() else {
        return false;
    };
    let probe = parent.join(".trurl-write-test");
    let ok = std::fs::write(&probe, b"x").is_ok();
    let _ = std::fs::remove_file(probe);
    ok
}

/// Download, verify, unpack, swap. Returns the path of the new executable.
pub async fn install(
    info: &UpdateInfo,
    progress: impl FnMut(u64, Option<u64>),
    core: &App,
) -> Result<PathBuf, UserError> {
    let bytes = update::download(&http(), info, progress).await?;
    let dir = app_dir().ok_or_else(UserError::update_failed)?;
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "app".into());
    let staging = dir.with_file_name(format!("{name}.new"));
    let old = dir.with_file_name(format!("{name}.old"));
    let _ = std::fs::remove_dir_all(&staging);
    let _ = std::fs::remove_dir_all(&old);
    let unpack = {
        let staging = staging.clone();
        tokio::task::spawn_blocking(move || unpack(&bytes, &staging)).await
    };
    let unpacked = match unpack {
        Ok(r) => r.map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    if let Err(e) = unpacked {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(UserError::update_failed().with_detail(e));
    }
    let exe = staging.join("trurl");
    if !exe.is_file() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(UserError::update_failed().with_detail("the update has no trurl program in it"));
    }
    if !staging.join(MARKER).is_file() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(UserError::update_failed().with_detail("the update is not a Trurl app folder"));
    }
    // An answer may have started while the update downloaded: never cut it off.
    if core.is_generating() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(UserError::update_wait_for_answer());
    }
    // From here on the old llama-server must not run from the old folder. If the
    // swap fails, the model comes back and Trurl carries on with the old version.
    core.release_engine().await;
    if let Err(e) = std::fs::rename(&dir, &old) {
        let _ = std::fs::remove_dir_all(&staging);
        core.reload();
        return Err(UserError::update_failed().with_detail(e.to_string()));
    }
    if let Err(e) = std::fs::rename(&staging, &dir) {
        let _ = std::fs::rename(&old, &dir);
        core.reload();
        return Err(UserError::update_failed().with_detail(e.to_string()));
    }
    let _ = std::fs::remove_dir_all(&old);
    Ok(dir.join("trurl"))
}

/// Unpack a `trurl-<v>-linux-x64.tar.gz` into `dest`, dropping its top-level
/// folder. `tar` refuses entries that would land outside the target folder.
fn unpack(bytes: &[u8], dest: &Path) -> std::io::Result<()> {
    let tmp = dest.with_extension("unpack");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    tar::Archive::new(flate2::read::GzDecoder::new(bytes)).unpack(&tmp)?;
    let entries: Vec<PathBuf> = std::fs::read_dir(&tmp)?
        .flatten()
        .map(|e| e.path())
        .collect();
    let root = match entries.as_slice() {
        [only] if only.is_dir() => only.clone(),
        _ => tmp.clone(),
    };
    std::fs::rename(&root, dest)?;
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(())
}

/// Replace this process with the new version.
pub fn restart(exe: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new(exe).exec();
        eprintln!("Trurl was updated. Start it again. ({err})");
    }
    #[cfg(not(unix))]
    {
        let _ = std::process::Command::new(exe).spawn();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn unpack_drops_the_top_folder() {
        let mut gz = Vec::new();
        {
            let enc = flate2::write::GzEncoder::new(&mut gz, flate2::Compression::fast());
            let mut b = tar::Builder::new(enc);
            for (name, data) in [
                ("trurl-0.2.0-linux-x64/trurl", "bin"),
                ("trurl-0.2.0-linux-x64/llama/compat/llama-server", "srv"),
            ] {
                let mut h = tar::Header::new_gnu();
                h.set_size(data.len() as u64);
                h.set_mode(0o755);
                h.set_cksum();
                b.append_data(&mut h, name, data.as_bytes()).unwrap();
            }
            b.into_inner().unwrap().finish().unwrap();
        }
        let d = tempfile::tempdir().unwrap();
        let dest = d.path().join("app.new");
        super::unpack(&gz, &dest).unwrap();
        assert_eq!(std::fs::read_to_string(dest.join("trurl")).unwrap(), "bin");
        assert_eq!(
            std::fs::read_to_string(dest.join("llama/compat/llama-server")).unwrap(),
            "srv"
        );
    }
}
