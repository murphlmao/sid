//! `kubectl config get-contexts -o name` → [`KubeContext`] and
//! `kubectl get pods -A -o json` → [`KubePod`] mapping.

use serde::Deserialize;
use sid_core::containers::{KubeContext, KubeError, KubePod};

/// Parse `kubectl config get-contexts -o name` stdout: one context name per line, no
/// indication of which is current (that's a separate `current-context` call — see
/// `client::current_context_raw` — combined here in [`build_contexts`]). Blank lines
/// are dropped; no context names at all is a valid, common state (freshly installed
/// kubectl with an empty kubeconfig) and yields an empty `Vec`, not an error.
pub(crate) fn parse_context_names(raw: &str) -> Vec<String> {
    raw.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// Zip parsed context `names` with the resolved `current` context name (if any) into
/// [`KubeContext`] rows.
pub(crate) fn build_contexts(names: Vec<String>, current: Option<&str>) -> Vec<KubeContext> {
    names
        .into_iter()
        .map(|name| {
            let is_current = current == Some(name.as_str());
            KubeContext {
                name,
                current: is_current,
            }
        })
        .collect()
}

/// `kubectl get pods -A -o json`'s top-level shape: a `List` with an `items` array.
#[derive(Deserialize)]
struct PodList {
    #[serde(default)]
    items: Vec<PodItem>,
}

#[derive(Deserialize)]
struct PodItem {
    metadata: PodMetadata,
    #[serde(default)]
    spec: PodSpec,
    #[serde(default)]
    status: PodStatus,
}

#[derive(Deserialize)]
struct PodMetadata {
    #[serde(default)]
    namespace: String,
    name: String,
}

#[derive(Deserialize, Default)]
struct PodSpec {
    #[serde(rename = "nodeName", default)]
    node_name: String,
}

#[derive(Deserialize, Default)]
struct PodStatus {
    #[serde(default)]
    phase: String,
    #[serde(rename = "containerStatuses", default)]
    container_statuses: Vec<ContainerStatus>,
}

#[derive(Deserialize)]
struct ContainerStatus {
    #[serde(default)]
    ready: bool,
    #[serde(rename = "restartCount", default)]
    restart_count: u32,
}

/// Parse `kubectl get pods -A -o json` stdout into [`KubePod`] rows. Never panics;
/// malformed JSON is a [`KubeError::Other`], not a panic. `ready` is computed as
/// `"<ready-count>/<total>"` from `status.containerStatuses`, matching `kubectl get
/// pods`'s own READY column; `restarts` sums `restartCount` across the pod's
/// containers. A pod with no `containerStatuses` yet (e.g. still `Pending`) reports
/// `"0/0"` and `0` restarts rather than erroring.
pub(crate) fn parse_pods_json(raw: &str) -> Result<Vec<KubePod>, KubeError> {
    let list: PodList =
        serde_json::from_str(raw).map_err(|e| KubeError::Other(format!("parse pods json: {e}")))?;
    Ok(list
        .items
        .into_iter()
        .map(|item| {
            let total = item.status.container_statuses.len();
            let ready_count = item
                .status
                .container_statuses
                .iter()
                .filter(|c| c.ready)
                .count();
            let restarts = item
                .status
                .container_statuses
                .iter()
                .map(|c| c.restart_count)
                .sum();
            KubePod {
                namespace: item.metadata.namespace,
                name: item.metadata.name,
                ready: format!("{ready_count}/{total}"),
                phase: item.status.phase,
                restarts,
                node: item.spec.node_name,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- context name parsing ----

    #[test]
    fn parses_multiple_context_names() {
        let raw = "minikube\ndocker-desktop\nprod-cluster\n";
        let names = parse_context_names(raw);
        assert_eq!(names, vec!["minikube", "docker-desktop", "prod-cluster"]);
    }

    #[test]
    fn blank_lines_are_dropped() {
        let raw = "minikube\n\n  \nprod\n";
        assert_eq!(parse_context_names(raw), vec!["minikube", "prod"]);
    }

    #[test]
    fn empty_output_is_an_empty_vec() {
        assert!(parse_context_names("").is_empty());
    }

    #[test]
    fn build_contexts_marks_the_current_one() {
        let names = vec!["a".to_string(), "b".to_string()];
        let contexts = build_contexts(names, Some("b"));
        assert_eq!(contexts.len(), 2);
        assert!(!contexts[0].current);
        assert!(contexts[1].current);
    }

    #[test]
    fn build_contexts_with_no_current_marks_none() {
        let names = vec!["a".to_string(), "b".to_string()];
        let contexts = build_contexts(names, None);
        assert!(contexts.iter().all(|c| !c.current));
    }

    // ---- pod JSON parsing ----

    #[test]
    fn parses_a_realistic_pod() {
        let json = r#"{
            "items": [{
                "metadata": {"namespace": "default", "name": "web-7f6b9c-abcde"},
                "spec": {"nodeName": "minikube"},
                "status": {
                    "phase": "Running",
                    "containerStatuses": [
                        {"ready": true, "restartCount": 2}
                    ]
                }
            }]
        }"#;
        let pods = parse_pods_json(json).unwrap();
        assert_eq!(pods.len(), 1);
        assert_eq!(pods[0].namespace, "default");
        assert_eq!(pods[0].name, "web-7f6b9c-abcde");
        assert_eq!(pods[0].ready, "1/1");
        assert_eq!(pods[0].phase, "Running");
        assert_eq!(pods[0].restarts, 2);
        assert_eq!(pods[0].node, "minikube");
    }

    #[test]
    fn multi_container_pod_sums_restarts_and_partial_ready() {
        let json = r#"{
            "items": [{
                "metadata": {"namespace": "ns", "name": "multi"},
                "spec": {"nodeName": "node-1"},
                "status": {
                    "phase": "Running",
                    "containerStatuses": [
                        {"ready": true, "restartCount": 1},
                        {"ready": false, "restartCount": 3}
                    ]
                }
            }]
        }"#;
        let pods = parse_pods_json(json).unwrap();
        assert_eq!(pods[0].ready, "1/2");
        assert_eq!(pods[0].restarts, 4);
    }

    #[test]
    fn pending_pod_with_no_container_statuses_is_zero_of_zero() {
        let json = r#"{
            "items": [{
                "metadata": {"namespace": "ns", "name": "pending-pod"},
                "spec": {},
                "status": {"phase": "Pending"}
            }]
        }"#;
        let pods = parse_pods_json(json).unwrap();
        assert_eq!(pods[0].ready, "0/0");
        assert_eq!(pods[0].restarts, 0);
        assert_eq!(pods[0].node, "");
    }

    #[test]
    fn empty_items_is_an_empty_vec() {
        assert!(parse_pods_json(r#"{"items": []}"#).unwrap().is_empty());
    }

    #[test]
    fn missing_items_key_defaults_to_empty_vec() {
        assert!(parse_pods_json("{}").unwrap().is_empty());
    }

    #[test]
    fn empty_string_input_is_an_error() {
        let e = parse_pods_json("").unwrap_err();
        assert!(matches!(e, KubeError::Other(_)));
    }

    #[test]
    fn malformed_json_is_other_error_not_panic() {
        let e = parse_pods_json("not json").unwrap_err();
        assert!(matches!(e, KubeError::Other(_)));
    }

    #[test]
    fn pod_missing_required_name_field_is_an_error() {
        let json = r#"{"items": [{"metadata": {"namespace": "ns"}, "status": {}}]}"#;
        let e = parse_pods_json(json).unwrap_err();
        assert!(matches!(e, KubeError::Other(_)));
    }

    #[test]
    fn pod_missing_namespace_defaults_to_empty() {
        let json = r#"{"items": [{"metadata": {"name": "a"}, "status": {}}]}"#;
        let pods = parse_pods_json(json).unwrap();
        assert_eq!(pods[0].namespace, "");
    }
}
