//! Fetch with a non-interactive credential policy and a hard time bound.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use sid_core::git::{FetchReport, GitError};

use crate::{ahead_behind, map_git2_error};

/// Upper bound for one fetch, connect + transfer. libgit2 offers no portable socket
/// timeout, so the work runs on a helper thread and is also cancelled cooperatively
/// from the progress/credential callbacks.
pub(crate) const FETCH_TIMEOUT: Duration = Duration::from_secs(90);

pub(crate) fn remotes(repo: &git2::Repository) -> Result<Vec<String>, GitError> {
    let names = repo.remotes().map_err(map_git2_error)?;
    Ok(names.iter().flatten().map(String::from).collect())
}

/// Under the repo lock: validate the remote and return the gitdir for the worker.
pub(crate) fn precheck(
    repo: &git2::Repository,
    remote: &str,
) -> Result<std::path::PathBuf, GitError> {
    // Fail fast (and with the right error) on an unknown remote before spawning.
    repo.find_remote(remote).map_err(|e| {
        if e.code() == git2::ErrorCode::NotFound {
            GitError::RemoteNotFound(remote.to_string())
        } else {
            map_git2_error(e)
        }
    })?;
    Ok(repo.path().to_path_buf())
}

/// Lock-free: run the fetch on a helper thread, bounded by `timeout`.
pub(crate) fn fetch(
    gitdir: std::path::PathBuf,
    remote: &str,
    timeout: Duration,
) -> Result<FetchReport, GitError> {
    let remote_name = remote.to_string();
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    {
        let cancel = Arc::clone(&cancel);
        std::thread::spawn(move || {
            let _ = tx.send(run_fetch(&gitdir, &remote_name, &cancel, timeout));
        });
    }
    // Grace so the in-thread deadline normally reports first, with its own message.
    match rx.recv_timeout(timeout + Duration::from_secs(5)) {
        Ok(r) => r,
        Err(_) => {
            cancel.store(true, Ordering::SeqCst);
            Err(GitError::TimedOut(format!("fetching '{remote}'")))
        }
    }
}

fn run_fetch(
    gitdir: &std::path::Path,
    remote_name: &str,
    cancel: &Arc<AtomicBool>,
    timeout: Duration,
) -> Result<FetchReport, GitError> {
    let repo = git2::Repository::open(gitdir).map_err(map_git2_error)?;
    let mut remote = repo.find_remote(remote_name).map_err(map_git2_error)?;
    let started = std::time::Instant::now();
    let cancel_flag = Arc::clone(cancel);
    let expired = move || started.elapsed() > timeout || cancel_flag.load(Ordering::SeqCst);

    let auth_failed = Arc::new(AtomicBool::new(false));
    let updated = Arc::new(Mutex::new(Vec::<String>::new()));
    let stats = Arc::new(Mutex::new((0usize, 0usize)));

    let mut callbacks = git2::RemoteCallbacks::new();
    {
        let auth_failed = Arc::clone(&auth_failed);
        let (mut tried_agent, mut tried_default) = (false, false);
        let expired = expired.clone();
        callbacks.credentials(move |_url, username, allowed| {
            if expired() {
                return Err(git2::Error::from_str("fetch timed out"));
            }
            // Each mechanism at most once: a rejected credential makes libgit2 call
            // back again, and an unbounded retry here would loop forever.
            if allowed.contains(git2::CredentialType::SSH_KEY) && !tried_agent {
                tried_agent = true;
                return git2::Cred::ssh_key_from_agent(username.unwrap_or("git"));
            }
            if allowed.contains(git2::CredentialType::DEFAULT) && !tried_default {
                tried_default = true;
                return git2::Cred::default();
            }
            auth_failed.store(true, Ordering::SeqCst);
            Err(git2::Error::from_str(
                "no usable credentials (ssh-agent / default only; sid never prompts)",
            ))
        });
    }
    {
        let stats = Arc::clone(&stats);
        let expired = expired.clone();
        callbacks.transfer_progress(move |p| {
            if let Ok(mut s) = stats.lock() {
                *s = (p.received_objects(), p.received_bytes());
            }
            !expired()
        });
    }
    {
        let updated = Arc::clone(&updated);
        callbacks.update_tips(move |name, _old, _new| {
            if let Ok(mut u) = updated.lock() {
                u.push(
                    name.strip_prefix("refs/remotes/")
                        .unwrap_or(name)
                        .to_string(),
                );
            }
            true
        });
    }
    let mut fo = git2::FetchOptions::new();
    fo.remote_callbacks(callbacks);

    let result = remote.fetch(&[] as &[&str], Some(&mut fo), None);
    drop(remote);
    if let Err(e) = result {
        return Err(
            if auth_failed.load(Ordering::SeqCst) || e.code() == git2::ErrorCode::Auth {
                GitError::AuthFailed(e.message().to_string())
            } else if expired() || cancel.load(Ordering::SeqCst) {
                GitError::TimedOut(format!("fetching '{remote_name}'"))
            } else {
                map_git2_error(e)
            },
        );
    }

    let (received_objects, received_bytes) = stats.lock().map(|s| *s).unwrap_or((0, 0));
    let updated_refs = updated.lock().map(|u| u.clone()).unwrap_or_default();
    let (ahead, behind) = match repo.head() {
        Ok(h) if h.is_branch() => match h.shorthand() {
            Some(n) => ahead_behind(&repo, n)?,
            None => (None, None),
        },
        _ => (None, None),
    };
    Ok(FetchReport {
        remote: remote_name.to_string(),
        received_objects,
        received_bytes,
        updated_refs,
        ahead,
        behind,
    })
}
