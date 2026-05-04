# Kubernetes Pods

A Pod is the smallest deployable unit in Kubernetes. It represents one or more
containers that share networking and storage, scheduled together on the same
node.

## Pod Lifecycle

Pods move through these phases:

- **Pending** -- accepted by the cluster but not yet running. The scheduler is
  finding a node, or images are being pulled.
- **Running** -- at least one container is running.
- **Succeeded** -- all containers terminated successfully (exit code 0).
- **Failed** -- at least one container terminated with a non-zero exit code.
- **Unknown** -- the pod state cannot be determined (usually a node
  communication failure).

## Multi-Container Patterns

While most pods run a single container, multi-container pods are common for:

- **Sidecar** -- auxiliary container that enhances the main container (e.g.
  log shipper, service mesh proxy).
- **Init container** -- runs to completion before the main container starts
  (e.g. database migration, config fetching).
- **Ambassador** -- proxy that simplifies access to external services.

## Resource Requests and Limits

Each container declares CPU and memory requirements:

```yaml
resources:
  requests:
    cpu: "250m"
    memory: "64Mi"
  limits:
    cpu: "500m"
    memory: "128Mi"
```

Requests guarantee minimum resources for scheduling. Limits cap usage --
exceeding memory limits triggers an OOMKill.

## Health Probes

Kubernetes uses three types of probes to manage container health:

- **Liveness** -- restart the container if it fails.
- **Readiness** -- remove the pod from service endpoints if it fails.
- **Startup** -- protect slow-starting containers from premature liveness
  checks.

Probes can be HTTP GET, TCP socket, gRPC, or exec-based.

## Pod Disruption Budgets

A PodDisruptionBudget (PDB) limits how many pods in a set can be
simultaneously unavailable during voluntary disruptions (node drain, cluster
upgrade). It ensures application availability during maintenance.
