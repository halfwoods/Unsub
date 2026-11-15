# Clearable BPM overrides

Date: 2026-11-10

## Context

Get Info must retain explicit clears when a file cannot be written. A rescan reads the file's old tags again, and an in-place Apple Music import may offer values for otherwise empty fields. Existing optional numeric overrides cannot distinguish a clear from no override.

## Decision

BPM uses `Option<Option<u16>>` in `TrackOverrides`: an absent JSON key means no pin, an integer pins a value, and JSON `null` pins a clear. Serialization omits the absent state; deserialization preserves explicit null. Old override records without BPM retain their existing meaning. Grouping and Comments follow Composer's existing `Option<String>` contract, where an empty string pins a clear.

Successful writes release the edited pin. Failed writes retain it. `filled_from` does not replace a pinned clear, and the scanner reapplies it when reading tags. BPM is normalized to 1 through 999 at tag/edit/import/sync boundaries.

## Alternatives rejected (why)

- A single optional BPM value loses the distinction between clearing and leaving tags authoritative.
- Serializing an absent BPM pin as null would turn unrelated overrides into clears after reopening the library.
- Changing every existing numeric override would expand this feature into a migration of unrelated edit semantics.

## Consequences

Future readers of stored overrides must preserve absent versus null for BPM. The public track value remains a nullable integer. Existing numeric fields retain their current behavior.
