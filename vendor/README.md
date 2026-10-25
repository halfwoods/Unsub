# macOS compatibility patches

These crates are copied from their published crates.io distributions, with the same versions
and their original license files. `Cargo.toml` uses `[patch.crates-io]` so every consumer shares
the fixes. The vendor directories are excluded from the workspace; ordinary workspace checks
do not run upstream tests that require audio hardware, keychain access or unpublished fixtures.

Original crates.io archive SHA-256 checksums:

- CPAL: `6f02e8d0327b42d3e2e4ab2119af397344eb9fc54a34bf0ddeaa1277af8681f1`
- Security Framework: `b7f4bc775c73d9a02cde8bf7b2ec4c9d12743edf609006c7facc23998404cd1d`

| Crate | Upstream revision | License | Change |
| --- | --- | --- | --- |
| `cpal` 0.18.2 | `e1612d5d98152f8dc2a62e1b51ef7cbf4f7f26b7` | Apache-2.0 (`cpal-0.18.2/LICENSE`) | Resolve CoreAudio process-tap functions at runtime and check macOS 14.2 before accessing `CATapDescription`. Playback and microphone capture retain their original paths on older systems. |
| `security-framework` 3.7.0 | `5f6e65114b77d5bc161d2b099cad09f2a67609d2` | MIT OR Apache-2.0 (`security-framework-3.7.0/LICENSE-*`) | Resolve `SecTrustEvaluateWithError` at runtime. On pre-10.14 systems use `SecTrustEvaluate` with the same policies, roots and date, accepting only `PROCEED` or `UNSPECIFIED`. |

`security-framework`'s `unsub-legacy-tests` feature only exposes the fallback for isolated
regression tests on newer macOS. The synthetic certificate fixtures in `tests/fixtures/`
have no surviving private keys and are never installed in a keychain. Tests fix the date,
disable network fetching, and restrict trust to their per-object anchors.

Run on macOS:

```sh
cargo test --manifest-path vendor/cpal-0.18.2/Cargo.toml --lib compatibility_tests
cargo test --manifest-path vendor/security-framework-3.7.0/Cargo.toml --features unsub-legacy-tests --test high_sierra
```

When upgrading either dependency, reapply or remove its patch after checking upstream support,
run these tests, and audit the final Intel executable for strong imports of
`AudioHardwareCreateProcessTap`, `AudioHardwareDestroyProcessTap` and
`SecTrustEvaluateWithError`. Lower deployment metadata alone cannot fix those imports.

The other upstream optional APIs remain unchanged. Enabling additional APIs (for example
certificate timestamp setters introduced in 10.14.2) requires another compatibility audit.
