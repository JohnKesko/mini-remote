//! Downloading, verifying and installing a release, then starting it.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::feed::agent;
use crate::{Release, UpdateError, sibling_temp_dir, verify};

/// A verified download, ready to install.
pub struct Downloaded {
    pub release: Release,
    pub file: PathBuf,
    dir: PathBuf,
}

impl Drop for Downloaded {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// What to start after installing.
pub struct Installed {
    pub version: String,
    relaunch: PathBuf,
}

/// Biggest download we accept.
const MAX_SIZE: u64 = 500 << 20;

/// Download the release file and its signature, and check the signature.
/// `progress` gets (bytes done, bytes total).
pub fn download(
    release: &Release,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &AtomicBool,
) -> Result<Downloaded, UpdateError> {
    let dir = std::env::temp_dir().join(format!("mini-remote-download-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir)?;
    let file = dir.join(&release.asset.name);
    let downloaded = Downloaded { release: release.clone(), file: file.clone(), dir };

    let agent = agent(None);
    let mut response = agent
        .get(&release.asset.url)
        .call()
        .map_err(|e| UpdateError::Network(e.to_string()))?;
    if response.status().as_u16() != 200 {
        return Err(UpdateError::Network(format!("download failed ({})", response.status())));
    }
    let total = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(release.asset.size);
    let mut reader = response.body_mut().with_config().limit(MAX_SIZE).reader();
    let mut out = fs::File::create(&file)?;
    let mut buf = vec![0u8; 128 * 1024];
    let mut done = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(UpdateError::Cancelled);
        }
        let n = reader.read(&mut buf).map_err(|e| UpdateError::Network(e.to_string()))?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        done += n as u64;
        progress(done, total.max(done));
    }
    out.flush()?;
    drop(out);

    let mut sig = agent
        .get(&release.signature.url)
        .call()
        .map_err(|e| UpdateError::Network(e.to_string()))?;
    if sig.status().as_u16() != 200 {
        return Err(UpdateError::Network(format!("signature download failed ({})", sig.status())));
    }
    let signature = sig
        .body_mut()
        .with_config()
        .limit(4096)
        .read_to_string()
        .map_err(|e| UpdateError::Network(e.to_string()))?;
    let contents = fs::read(&file)?;
    if !verify(&release.version, &release.asset.name, &contents, &signature) {
        log::warn!("signature check failed for {}", release.asset.name);
        return Err(UpdateError::BadSignature);
    }
    Ok(downloaded)
}

/// Replace the running app with the downloaded one.
pub fn install(downloaded: &Downloaded) -> Result<Installed, UpdateError> {
    let exe = std::env::current_exe()?.canonicalize()?;
    let relaunch = install_for_platform(&exe, &downloaded.file)?;
    Ok(Installed { version: downloaded.release.version.clone(), relaunch })
}

#[cfg(target_os = "macos")]
fn install_for_platform(exe: &Path, archive: &Path) -> Result<PathBuf, UpdateError> {
    let bundle = exe
        .ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .ok_or_else(|| UpdateError::NotInstallable("Mini Remote is not running from an app (for example a test build)".into()))?
        .to_path_buf();
    let tmp = sibling_temp_dir(&bundle).map_err(|e| not_writable(&bundle, e))?;
    let result = (|| {
        let status = Command::new("/usr/bin/ditto").arg("-x").arg("-k").arg(archive).arg(&tmp).status()?;
        if !status.success() {
            return Err(UpdateError::Install("the download could not be unpacked".into()));
        }
        let new_bundle = fs::read_dir(&tmp)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|e| e == "app"))
            .ok_or_else(|| UpdateError::Install("the download does not contain the app".into()))?;
        replace(&new_bundle, &bundle)
    })();
    let _ = fs::remove_dir_all(&tmp);
    result.map(|()| bundle)
}

#[cfg(target_os = "linux")]
fn install_for_platform(exe: &Path, archive: &Path) -> Result<PathBuf, UpdateError> {
    use std::os::unix::fs::PermissionsExt;
    let tmp = sibling_temp_dir(exe).map_err(|e| not_writable(exe, e))?;
    let result = (|| {
        let gz = flate2::read::GzDecoder::new(fs::File::open(archive)?);
        // `unpack` refuses entries that would land outside the folder.
        tar::Archive::new(gz).unpack(&tmp)?;
        let new_exe = tmp.join("mini-remote").join("mini-remote");
        if !new_exe.is_file() {
            return Err(UpdateError::Install("the download does not contain the app".into()));
        }
        fs::set_permissions(&new_exe, fs::Permissions::from_mode(0o755))?;
        // Renaming over a running program is fine on Linux.
        fs::rename(&new_exe, exe)?;
        Ok(())
    })();
    let _ = fs::remove_dir_all(&tmp);
    result.map(|()| exe.to_path_buf())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn install_for_platform(_exe: &Path, _archive: &Path) -> Result<PathBuf, UpdateError> {
    Err(UpdateError::NotInstallable("automatic updates are not available on this system".into()))
}

/// Move `new` to `old`'s place, keeping `old` until the move worked.
#[cfg(target_os = "macos")]
fn replace(new: &Path, old: &Path) -> Result<(), UpdateError> {
    let name = old.file_name().and_then(|n| n.to_str()).unwrap_or("app");
    let backup = old.with_file_name(format!(".{name}.old-{}", std::process::id()));
    fs::rename(old, &backup).map_err(|e| not_writable(old, e))?;
    if let Err(e) = fs::rename(new, old) {
        let _ = fs::rename(&backup, old);
        return Err(UpdateError::Install(e.to_string()));
    }
    let _ = fs::remove_dir_all(&backup);
    Ok(())
}

fn not_writable(path: &Path, e: std::io::Error) -> UpdateError {
    let folder = path.parent().unwrap_or(path).display();
    UpdateError::Install(format!("Mini Remote can't write to {folder} ({e}). Move it to a folder you own and try again."))
}

/// Start the new version once this process has quit. Call this, then quit
/// the app normally.
pub fn schedule_restart(installed: &Installed) -> Result<(), UpdateError> {
    let pid = std::process::id().to_string();
    let target = installed.relaunch.as_os_str();
    // Wait for this process to end, then start the new version.
    let script = if cfg!(target_os = "macos") {
        r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done; exec /usr/bin/open -n "$2""#
    } else {
        r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done; exec "$2" </dev/null >/dev/null 2>&1"#
    };
    Command::new("/bin/sh")
        .arg("-c")
        .arg(script)
        .arg("mini-remote-restart")
        .arg(&pid)
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    log::info!("installed version {}; restarting", installed.version);
    Ok(())
}
