//! Local SSH identity discovery: the port, and the pure rules that decide what counts
//! as a key and which one is the conventional choice. Implementations live in `sid-ssh`.
//!
//! # Why this is a port and not three lines of `read_dir`
//!
//! The add-connection form needs to answer one question — *what can this machine
//! authenticate with?* — and the honest answer involves the filesystem and the process
//! environment. Both are I/O, so both cross a seam (CLAUDE.md rule 1): the form states
//! the need as [`IdentityScan`], and `sid-ssh` owns the `~/.ssh` + `SSH_AUTH_SOCK`
//! mechanics. Swapping in [`StaticIdentities`] is what lets the ranking, the
//! preselection and the empty-scan path be tested without a home directory.
//!
//! # The privacy invariant
//!
//! **A scan never reads private key material.** A candidate is identified from its file
//! *name*, its *mode bits* and the presence of a `.pub` sibling — see [`KeyEvidence`],
//! which is the entire input to the decision. No implementation of this port may open a
//! private key, and nothing here ever logs a key's contents.

use std::path::PathBuf;

/// A private key sid is willing to offer as an identity file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyCandidate {
    /// Absolute path to the private key.
    pub path: PathBuf,
    /// The file name alone, e.g. `id_ed25519` — what a picker shows.
    pub name: String,
    /// A matching `<name>.pub` sits beside it. Strong evidence, since a lone public key
    /// is the one artefact of a key pair that is safe to look at.
    pub has_public_sibling: bool,
}

/// What a scanner observed about one file in the key directory — the complete input to
/// [`is_key_candidate`], and deliberately nothing that requires opening the file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyEvidence<'a> {
    /// The file name, no directory part.
    pub name: &'a str,
    /// It is a regular file (not a directory, socket or symlink to nowhere).
    pub is_file: bool,
    /// Its mode bits deny group and other every access — `chmod 600`, what OpenSSH
    /// insists on for a private key. `false` where a platform can't say.
    pub owner_only: bool,
    /// A `<name>.pub` exists in the same directory.
    pub has_public_sibling: bool,
}

/// Why a scan produced nothing.
#[derive(Debug, thiserror::Error)]
pub enum KeyScanError {
    /// There is no key directory at all — a machine that has never run `ssh-keygen`.
    #[error("no ssh directory at {0}")]
    NoKeyDir(PathBuf),
    /// The directory exists but could not be listed.
    #[error("cannot read {path}: {reason}")]
    Unreadable {
        /// The directory that could not be listed.
        path: PathBuf,
        /// The OS's reason, verbatim.
        reason: String,
    },
    /// This machine has no home directory to look in.
    #[error("no home directory to look for ssh keys in")]
    NoHome,
}

/// What this machine can authenticate to an SSH server with.
///
/// One port rather than two because the caller asks one question and acts on the whole
/// answer: with an agent up, agent auth is the right default; with no agent and a key on
/// disk, defaulting to the agent is a guaranteed failure — see
/// [`preferred_auth`](crate::keys::preferred_auth).
pub trait IdentityScan: Send + Sync {
    /// The directory keys are looked for in — named in the picker so "no keys found"
    /// says *where* nothing was found.
    fn key_dir(&self) -> PathBuf;

    /// Every plausible private key in [`Self::key_dir`], best-first (see [`rank`]).
    fn keys(&self) -> Result<Vec<KeyCandidate>, KeyScanError>;

    /// Whether an ssh-agent is reachable from this process *right now*.
    ///
    /// Not "was one running when you logged in": a desktop-launched app inherits the
    /// session's environment as it was at login, which is exactly how sid ended up
    /// defaulting every new host to an agent that was never there.
    fn agent_available(&self) -> bool;
}

/// A fixed answer — the in-memory implementation, for tests and for platforms that have
/// no key directory to offer yet.
#[derive(Clone, Debug, Default)]
pub struct StaticIdentities {
    /// What [`IdentityScan::key_dir`] reports.
    pub dir: PathBuf,
    /// What [`IdentityScan::keys`] reports (already ranked by the constructor).
    pub keys: Vec<KeyCandidate>,
    /// What [`IdentityScan::agent_available`] reports.
    pub agent: bool,
}

impl IdentityScan for StaticIdentities {
    fn key_dir(&self) -> PathBuf {
        self.dir.clone()
    }

    fn keys(&self) -> Result<Vec<KeyCandidate>, KeyScanError> {
        Ok(self.keys.clone())
    }

    fn agent_available(&self) -> bool {
        self.agent
    }
}

/// Where a key name sits in the conventional pecking order. Declaration order **is** the
/// preference order — `id_ed25519` is what `ssh-keygen` has produced by default for
/// years, `id_rsa` is what it produced before that, and everything else is a key someone
/// named deliberately and can pick out of the list themselves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum KeyRank {
    /// Exactly `id_ed25519`.
    Ed25519,
    /// An ed25519 key with a suffix, e.g. `id_ed25519_work`.
    Ed25519Variant,
    /// Exactly `id_rsa`.
    Rsa,
    /// An rsa key with a suffix, e.g. `id_rsa_legacy`.
    RsaVariant,
    /// Anything else.
    Other,
}

/// Files that live in `~/.ssh` and are never private keys, whatever their mode bits say.
/// `known_hosts` is matched by prefix so `known_hosts.old` is excluded too.
const NEVER_A_KEY: [&str; 5] = [
    "config",
    "authorized_keys",
    "authorized_keys2",
    "environment",
    "rc",
];

/// Could a file with this name be a private key?
///
/// Name-only, and deliberately so: this is the half of the decision that needs no
/// filesystem at all. Rejects the public half of a pair (`*.pub`), OpenSSH's own
/// bookkeeping files, and dotfiles.
pub fn looks_like_private_key(name: &str) -> bool {
    if name.is_empty() || name.starts_with('.') || name.ends_with(".pub") {
        return false;
    }
    if name.starts_with("known_hosts") {
        return false;
    }
    !NEVER_A_KEY.contains(&name)
}

/// Is this file a private key worth offering?
///
/// The name rule plus one piece of corroboration, because `~/.ssh` collects junk: a
/// public sibling, owner-only permissions, or the conventional `id_` prefix. Any one of
/// the three is enough; none of them requires reading a byte of the file.
pub fn is_key_candidate(evidence: &KeyEvidence<'_>) -> bool {
    evidence.is_file
        && looks_like_private_key(evidence.name)
        && (evidence.has_public_sibling || evidence.owner_only || evidence.name.starts_with("id_"))
}

/// Where `name` sits in the conventional pecking order.
pub fn rank(name: &str) -> KeyRank {
    match name {
        "id_ed25519" => KeyRank::Ed25519,
        "id_rsa" => KeyRank::Rsa,
        n if n.starts_with("id_ed25519") => KeyRank::Ed25519Variant,
        n if n.starts_with("id_rsa") => KeyRank::RsaVariant,
        _ => KeyRank::Other,
    }
}

/// Sort candidates best-first: by [`rank`], then by name so the order is stable across
/// scans (a picker that reshuffles between openings is a picker nobody trusts).
pub fn sort_candidates(candidates: &mut [KeyCandidate]) {
    candidates.sort_by(|a, b| rank(&a.name).cmp(&rank(&b.name)).then(a.name.cmp(&b.name)));
}

/// The key an add form should fill in without being asked. `None` when the scan found
/// nothing — the caller then offers the picker rather than failing (issue #2).
pub fn preferred(candidates: &[KeyCandidate]) -> Option<&KeyCandidate> {
    candidates
        .iter()
        .min_by(|a, b| rank(&a.name).cmp(&rank(&b.name)).then(a.name.cmp(&b.name)))
}

/// Which auth method a *new* host should open on.
///
/// Agent auth stays the default whenever an agent is actually reachable — it is the
/// method that stores no secret and needs no path. With no agent, the old default was a
/// guaranteed failure at connect time ("SSH_AUTH_SOCK not set", issue #2), so a key on
/// disk wins instead. With neither, it stays on agent: that is the honest "nothing
/// configured yet" state, and the form's own copy explains it.
pub fn preferred_auth(agent_available: bool, has_key: bool) -> PreferredAuth {
    match (agent_available, has_key) {
        (true, _) => PreferredAuth::Agent,
        (false, true) => PreferredAuth::Key,
        (false, false) => PreferredAuth::Agent,
    }
}

/// The answer [`preferred_auth`] gives — the domain's half of the UI's auth selector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreferredAuth {
    /// Use the running ssh-agent.
    Agent,
    /// Use a private key from the scan.
    Key,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(name: &str) -> KeyCandidate {
        KeyCandidate {
            path: PathBuf::from("/home/u/.ssh").join(name),
            name: name.to_string(),
            has_public_sibling: true,
        }
    }

    fn evidence(name: &str) -> KeyEvidence<'_> {
        KeyEvidence {
            name,
            is_file: true,
            owner_only: true,
            has_public_sibling: true,
        }
    }

    // ---- the name filter ---------------------------------------------------------

    #[test]
    fn the_public_half_of_a_pair_is_not_a_key() {
        assert!(!looks_like_private_key("id_ed25519.pub"));
        assert!(looks_like_private_key("id_ed25519"));
    }

    #[test]
    fn openssh_bookkeeping_files_are_not_keys() {
        for name in [
            "config",
            "known_hosts",
            "known_hosts.old",
            "authorized_keys",
            "authorized_keys2",
            "environment",
            "rc",
        ] {
            assert!(!looks_like_private_key(name), "{name} read as a key");
        }
    }

    #[test]
    fn dotfiles_and_the_empty_name_are_not_keys() {
        assert!(!looks_like_private_key(".gitignore"));
        assert!(!looks_like_private_key(""));
    }

    #[test]
    fn a_deliberately_named_key_is_still_a_key() {
        // The scan must not be a whitelist of two names — people name keys after the
        // machine they open.
        assert!(looks_like_private_key("vps-1"));
        assert!(looks_like_private_key("work_deploy"));
    }

    // ---- the corroboration rule ---------------------------------------------------

    #[test]
    fn a_directory_is_never_a_candidate() {
        let mut e = evidence("id_ed25519");
        e.is_file = false;
        assert!(!is_key_candidate(&e));
    }

    #[test]
    fn a_public_sibling_is_enough_on_its_own() {
        let e = KeyEvidence {
            name: "vps-1",
            is_file: true,
            owner_only: false,
            has_public_sibling: true,
        };
        assert!(is_key_candidate(&e));
    }

    #[test]
    fn owner_only_permissions_are_enough_on_their_own() {
        let e = KeyEvidence {
            name: "vps-1",
            is_file: true,
            owner_only: true,
            has_public_sibling: false,
        };
        assert!(is_key_candidate(&e));
    }

    #[test]
    fn the_conventional_prefix_is_enough_on_its_own() {
        // A key restored from a backup can arrive world-readable and without its `.pub`;
        // `id_` is still what it is.
        let e = KeyEvidence {
            name: "id_ecdsa",
            is_file: true,
            owner_only: false,
            has_public_sibling: false,
        };
        assert!(is_key_candidate(&e));
    }

    #[test]
    fn an_uncorroborated_stray_file_is_not_offered() {
        // `~/.ssh` collects junk — a world-readable note with no pair beside it is not
        // a key, and offering it would be noise in the picker.
        let e = KeyEvidence {
            name: "notes.txt",
            is_file: true,
            owner_only: false,
            has_public_sibling: false,
        };
        assert!(!is_key_candidate(&e));
    }

    #[test]
    fn corroboration_never_overrides_the_name_rule() {
        // `known_hosts` is routinely 0600 — mode bits must not promote it to a key.
        let e = KeyEvidence {
            name: "known_hosts",
            is_file: true,
            owner_only: true,
            has_public_sibling: false,
        };
        assert!(!is_key_candidate(&e));
    }

    // ---- ranking -------------------------------------------------------------------

    #[test]
    fn ed25519_outranks_rsa_outranks_everything_else() {
        assert!(rank("id_ed25519") < rank("id_rsa"));
        assert!(rank("id_rsa") < rank("vps-1"));
    }

    #[test]
    fn a_suffixed_key_sits_just_below_its_plain_form() {
        assert!(rank("id_ed25519") < rank("id_ed25519_work"));
        assert!(rank("id_ed25519_work") < rank("id_rsa"));
        assert!(rank("id_rsa") < rank("id_rsa_legacy"));
        assert!(rank("id_rsa_legacy") < rank("vps-1"));
    }

    #[test]
    fn the_scan_is_ordered_best_first() {
        let mut found = vec![
            candidate("vps-1"),
            candidate("id_rsa"),
            candidate("id_ed25519_work"),
            candidate("id_ed25519"),
        ];
        sort_candidates(&mut found);
        let names: Vec<&str> = found.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["id_ed25519", "id_ed25519_work", "id_rsa", "vps-1"]
        );
    }

    #[test]
    fn equally_ranked_keys_keep_a_stable_alphabetical_order() {
        let mut found = vec![candidate("zeta"), candidate("alpha"), candidate("mid")];
        sort_candidates(&mut found);
        let names: Vec<&str> = found.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "mid", "zeta"]);
    }

    #[test]
    fn the_conventional_key_is_the_one_preselected() {
        let found = vec![
            candidate("vps-1"),
            candidate("id_rsa"),
            candidate("id_ed25519"),
        ];
        assert_eq!(
            preferred(&found).map(|c| c.name.as_str()),
            Some("id_ed25519")
        );
    }

    #[test]
    fn preselection_does_not_depend_on_the_list_already_being_sorted() {
        // `preferred` is read straight off an adapter's output in the UI; pinning it to
        // the same rule as `sort_candidates` keeps the picker's first row and the
        // prefilled path from ever disagreeing.
        let mut found = vec![
            candidate("vps-1"),
            candidate("id_rsa"),
            candidate("id_ed25519"),
        ];
        let best = preferred(&found).cloned();
        sort_candidates(&mut found);
        assert_eq!(best.as_ref(), found.first());
    }

    #[test]
    fn nothing_found_preselects_nothing() {
        assert_eq!(preferred(&[]), None);
    }

    // ---- the auth default ----------------------------------------------------------

    #[test]
    fn an_available_agent_stays_the_default() {
        assert_eq!(preferred_auth(true, true), PreferredAuth::Agent);
        assert_eq!(preferred_auth(true, false), PreferredAuth::Agent);
    }

    #[test]
    fn no_agent_but_a_key_on_disk_defaults_to_the_key() {
        // The bug behind issue #2: `AuthMethod::Agent` is `#[default]`, a desktop-
        // launched sid inherits no `SSH_AUTH_SOCK`, and every new host was therefore
        // born pointing at an agent that did not exist.
        assert_eq!(preferred_auth(false, true), PreferredAuth::Key);
    }

    #[test]
    fn no_agent_and_no_key_still_lands_on_agent() {
        // Nothing to prefill a key path with, so the honest state is the method that
        // needs no path — the form's copy is what explains it.
        assert_eq!(preferred_auth(false, false), PreferredAuth::Agent);
    }

    // ---- the port ------------------------------------------------------------------

    #[test]
    fn the_static_source_answers_what_it_was_given() {
        let src = StaticIdentities {
            dir: PathBuf::from("/home/u/.ssh"),
            keys: vec![candidate("id_ed25519")],
            agent: true,
        };
        assert_eq!(src.key_dir(), PathBuf::from("/home/u/.ssh"));
        assert_eq!(src.keys().unwrap().len(), 1);
        assert!(src.agent_available());
    }

    #[allow(dead_code)]
    fn assert_object_safe(_s: &dyn IdentityScan) {}
}
