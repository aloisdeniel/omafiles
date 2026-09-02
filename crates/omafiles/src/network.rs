//! Network locations — the sidebar's NETWORK section.
//!
//! Built on GVfs rather than on anything of our own: `gio mount <uri>` does
//! the mounting (SMB, SFTP, FTP, WebDAV — whatever the user's GVfs speaks),
//! and the mount surfaces as a FUSE directory under `$XDG_RUNTIME_DIR/gvfs/`,
//! which the listing, the preview and the finder can then browse like any
//! other path with no special handling anywhere. This module only remembers
//! the locations, mounts them, and finds where the mount landed.
//!
//! The list lives in `~/.config/omafiles/network.toml` — config, not state,
//! because it is user-curated like `places.toml`, and written with the same
//! atomic discipline.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Location {
    /// A label; derived from the URI when the user gives none.
    pub name: String,
    /// `smb://host/share`, `sftp://user@host/path`, `dav(s)://…`, `ftp://…`.
    pub uri: String,
}

/// A label a person would use: `share on host` for a share, `host` alone
/// otherwise.
pub fn derive_name(uri: &str) -> String {
    let rest = uri.split_once("://").map(|(_, r)| r).unwrap_or(uri);
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    // The user@ half is a credential, not an identity worth a label.
    let host = authority.rsplit_once('@').map(|(_, h)| h).unwrap_or(authority);
    let first = path.split('/').find(|part| !part.is_empty());
    match first {
        Some(share) => format!("{share} on {host}"),
        None => host.to_string(),
    }
}

/// The host inside a URI — what a GVfs mount directory names itself after.
fn host_of(uri: &str) -> Option<String> {
    let rest = uri.split_once("://")?.1;
    let authority = rest.split('/').next()?;
    let host = authority.rsplit_once('@').map(|(_, h)| h).unwrap_or(authority);
    let host = host.split(':').next()?; // drop a :port
    (!host.is_empty()).then(|| host.to_string())
}

/// `$XDG_RUNTIME_DIR/gvfs`, where GVfs surfaces its mounts as directories.
fn gvfs_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/run/user/1000"))
        .join("gvfs")
}

/// Where `uri`'s mount landed, if it is mounted.
///
/// GVfs names its FUSE directories from the scheme, host and share
/// (`smb-share:server=host,share=x`, `sftp:host=host,…`), so matching on the
/// host — and the share's first path segment when there is one — finds the
/// right one without reimplementing GVfs's own escaping rules.
pub fn mount_point(uri: &str) -> Option<PathBuf> {
    mount_point_in(&gvfs_dir(), uri)
}

fn mount_point_in(dir: &Path, uri: &str) -> Option<PathBuf> {
    let host = host_of(uri)?;
    let share = uri
        .split_once("://")
        .and_then(|(_, rest)| rest.split_once('/'))
        .map(|(_, path)| path.split('/').find(|p| !p.is_empty()).unwrap_or(""))
        .unwrap_or("");

    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            name.contains(&format!("host={host}"))
                || name.contains(&format!("server={host}"))
                    && (share.is_empty() || name.contains(&format!("share={share}")))
        })
}

/// Mount `uri` through GVfs. **Blocking** — background executors only.
///
/// `gio mount` succeeds silently when the location is already mounted, so
/// callers need no is-it-mounted dance. Credentials come from the keyring;
/// with none saved, gio's refusal is surfaced verbatim — the terminal (or a
/// guest share) is the v1 answer for interactive authentication.
pub fn mount(uri: &str) -> Result<(), String> {
    let output = Command::new("gio")
        .arg("mount")
        .arg(uri)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("could not run gio: {err}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    // Already mounted is success by another name.
    if stderr.contains("already mounted") {
        return Ok(());
    }
    Err(if stderr.is_empty() {
        format!("could not mount {uri}")
    } else {
        stderr
    })
}

/// Unmount through GVfs. **Blocking** — background executors only.
pub fn unmount(uri: &str) -> Result<(), String> {
    let output = Command::new("gio")
        .arg("mount")
        .arg("-u")
        .arg(uri)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("could not run gio: {err}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(if stderr.is_empty() {
        format!("could not unmount {uri}")
    } else {
        stderr
    })
}

/// A quick sanity check before saving: a scheme and a host, nothing more.
pub fn looks_like_uri(text: &str) -> bool {
    text.split_once("://")
        .is_some_and(|(scheme, _)| !scheme.is_empty() && host_of(text).is_some())
}

/// Load the saved locations. Missing file is an empty list; a corrupt one
/// loads empty and is left on disk, the `places.toml` discipline.
pub fn load(config_dir: &Path) -> Vec<Location> {
    let path = config_dir.join("omafiles/network.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    #[derive(serde::Deserialize)]
    struct File {
        #[serde(default)]
        location: Vec<Location>,
    }
    toml::from_str::<File>(&text)
        .map(|file| file.location)
        .unwrap_or_default()
}

/// Persist atomically: temp + `sync_all` + rename, like `places.toml`.
pub fn save(config_dir: &Path, locations: &[Location]) -> std::io::Result<()> {
    #[derive(serde::Serialize)]
    struct File<'a> {
        location: &'a [Location],
    }
    let body = toml::to_string_pretty(&File { location: locations }).unwrap_or_default();
    let dir = config_dir.join("omafiles");
    std::fs::create_dir_all(&dir)?;
    let tmp = dir.join("network.toml.tmp");
    {
        let mut file = std::fs::File::create(&tmp)?;
        use std::io::Write as _;
        file.write_all(body.as_bytes())?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, dir.join("network.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_read_like_a_person_would_say_them() {
        assert_eq!(derive_name("smb://nas.local/media"), "media on nas.local");
        assert_eq!(derive_name("sftp://alois@server/home/alois"), "home on server");
        assert_eq!(derive_name("ftp://ftp.example.org"), "ftp.example.org");
        assert_eq!(derive_name("davs://cloud.example.org/dav/files"), "dav on cloud.example.org");
    }

    #[test]
    fn uri_sanity_wants_a_scheme_and_a_host() {
        assert!(looks_like_uri("smb://nas/share"));
        assert!(looks_like_uri("sftp://user@host:2222/dir"));
        assert!(!looks_like_uri("nas/share"));
        assert!(!looks_like_uri("://nothing"));
        assert!(!looks_like_uri("smb://"));
    }

    #[test]
    fn finds_the_mount_gvfs_created() {
        let dir = std::env::temp_dir().join(format!("omafiles-gvfs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("smb-share:server=nas.local,share=media")).unwrap();
        std::fs::create_dir_all(dir.join("sftp:host=devbox,user=alois")).unwrap();

        assert_eq!(
            mount_point_in(&dir, "smb://nas.local/media"),
            Some(dir.join("smb-share:server=nas.local,share=media"))
        );
        assert_eq!(
            mount_point_in(&dir, "sftp://alois@devbox/home"),
            Some(dir.join("sftp:host=devbox,user=alois"))
        );
        assert_eq!(mount_point_in(&dir, "smb://elsewhere/x"), None);
    }

    #[test]
    fn the_list_round_trips_and_survives_corruption() {
        let dir = std::env::temp_dir().join(format!("omafiles-nettoml-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let locations = vec![
            Location {
                name: "media on nas".to_string(),
                uri: "smb://nas.local/media".to_string(),
            },
            Location {
                name: "devbox".to_string(),
                uri: "sftp://alois@devbox/".to_string(),
            },
        ];
        save(&dir, &locations).unwrap();
        assert_eq!(load(&dir), locations);

        std::fs::write(dir.join("omafiles/network.toml"), "not [ toml").unwrap();
        assert!(load(&dir).is_empty());
        // The broken file is left for the user to fix, not clobbered.
        assert!(dir.join("omafiles/network.toml").exists());
    }
}
