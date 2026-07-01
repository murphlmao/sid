//! P2.1 — serde/postcard round-trips for every entity and for `Scope`.

use sid_store::codec::{decode_versioned, encode_versioned};
use sid_store::{DbConnection, Host, QuickAction, Scope, WorkspaceId};

fn sample_host() -> Host {
    Host {
        alias: "prod".into(),
        user: "deploy".into(),
        host: "prod.acme-api.internal".into(),
        port: 22,
        secret_ref: Some("ssh.prod.key".into()),
    }
}

#[test]
fn host_roundtrip_preserves_all_fields_with_version_byte() {
    let h = sample_host();
    let bytes = encode_versioned(1, &h).unwrap();
    assert_eq!(bytes[0], 1, "leading byte is the version");
    let (version, got): (u8, Host) = decode_versioned(&bytes).unwrap();
    assert_eq!(version, 1);
    assert_eq!(got, h);
}

#[test]
fn host_secret_ref_is_optional() {
    let h = Host {
        alias: "x".into(),
        user: "u".into(),
        host: "h".into(),
        port: 2222,
        secret_ref: None,
    };
    let (_, got): (u8, Host) = decode_versioned(&encode_versioned(1, &h).unwrap()).unwrap();
    assert_eq!(got.secret_ref, None);
    assert_eq!(got, h);
}

#[test]
fn db_connection_roundtrip() {
    let c = DbConnection {
        id: "acme-pg".into(),
        dsn: "postgres://acme@db.acme.internal/acme".into(),
        secret_ref: Some("db.acme-pg.pw".into()),
    };
    let (_, got): (u8, DbConnection) =
        decode_versioned(&encode_versioned(3, &c).unwrap()).unwrap();
    assert_eq!(got, c);
}

#[test]
fn quick_action_roundtrip() {
    let q = QuickAction {
        label: "tail app log".into(),
        cmd: "journalctl -u acme-api -f".into(),
    };
    let (_, got): (u8, QuickAction) =
        decode_versioned(&encode_versioned(1, &q).unwrap()).unwrap();
    assert_eq!(got, q);
}

#[test]
fn scope_roundtrip() {
    let s = Scope::Workspace(WorkspaceId("/home/murphy/code/acme/acme-api".into()));
    let (_, got): (u8, Scope) = decode_versioned(&encode_versioned(1, &s).unwrap()).unwrap();
    assert_eq!(got, s);
    let g = Scope::Global;
    let (_, got_g): (u8, Scope) = decode_versioned(&encode_versioned(1, &g).unwrap()).unwrap();
    assert_eq!(got_g, g);
}

#[test]
fn decode_empty_payload_errors() {
    let r = decode_versioned::<Host>(&[]);
    assert!(r.is_err(), "empty input must not decode");
}
