# Changelog — ironsbe-server

## 0.4.0

### Added
- `ServerHandle::send_to(session_id, message)` — server-initiated **unicast**
  push to a single session, complementing `broadcast` (all sessions). Backed by a
  new `ServerCommand::SendTo(u64, Vec<u8>)` handled in the run loop, which resolves
  the target in the live session-sender registry and opportunistically reaps a
  closed channel (mirroring `Broadcast`). A missing session id is a benign no-op.

  Enables consumers to drive subscription-gated server push (e.g. account-manager's
  live `MmEligibilityChanged` stream) without broadcasting to every session.

### Changed (breaking)
- `ServerCommand` is now `#[non_exhaustive]`. Downstream code matching on it
  must add a wildcard arm. This makes future command additions non-breaking
  (minor releases). Minor bump under 0.x semver reflects the break.
