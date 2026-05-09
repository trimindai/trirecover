# TriRecover — Threat Model

Scope: a read-only desktop forensics application that reads raw disk sectors with administrator privileges on Windows.

## Assets

| Asset                  | Why it matters                                                     |
| ---------------------- | ------------------------------------------------------------------ |
| Source drive contents  | The user's only copy of the data. Corruption is unrecoverable.     |
| Scan session DB        | Hours of CPU/IO work. Loss = restart the scan.                     |
| Recovered output files | The final user-visible product.                                    |
| User installation key  | Future commercial licensing — must not leak across machines.       |

## Trust boundaries

1. **Untrusted disk image**: the source drive is treated as adversarial input. Every byte parsed must be range-checked.
2. **Trusted local user**: the user is the owner of the data and runs the app. We do not defend against the local administrator.
3. **Untrusted network**: update server, telemetry, and license server are over TLS-pinned HTTPS.

## Threats and mitigations

### T1 — Buggy parser writes to source drive
**Likelihood:** low. **Impact:** catastrophic.
**Mitigations:**
- OS handle is `GENERIC_READ` only. Even a 100 % code bug cannot write.
- `SectorReader` trait has no write method. There is no API path.
- Code review rule: any `unsafe` outside `crates/storage/{windows,linux}.rs` is rejected.

### T2 — Malicious crafted on-disk metadata triggers memory corruption
**Likelihood:** medium (if user scans an attacker-supplied disk image).
**Impact:** RCE in a privileged process.
**Mitigations:**
- All parsers are pure Rust with `#![forbid(unsafe_code)]`.
- Reads use `zerocopy` validated layouts, never raw transmute.
- Lengths from disk are clamped to a per-record budget (`MAX_ATTR_LEN`, `MAX_RUNLIST_LEN`, etc.).
- Fuzz targets in `tests/fuzz/` (cargo-fuzz) cover MBR, GPT, NTFS MFT, FAT directory.

### T3 — Recovery overwrites the very files being recovered
**Likelihood:** medium (default UX hazard).
**Impact:** data loss.
**Mitigations:**
- Hard guard: `engine::recover` refuses if `same_volume(src, dest)`.
- UI also refuses in the destination picker before sending the IPC.
- Documentation calls this out on first run.

### T4 — Privilege escalation via the Tauri shell
**Likelihood:** low.
**Impact:** local LPE.
**Mitigations:**
- Tauri 2 capabilities allow-list pinned in `src-tauri/capabilities/`.
- No shell command execution from frontend.
- No file dialog auto-execution (recovered binaries are never launched by the app).

### T5 — Telemetry exfiltrates user data
**Likelihood:** low.
**Impact:** privacy violation.
**Mitigations:**
- Telemetry **off by default** in `Settings`.
- When on, payload is fixed schema: `{ build, os, anon_install_id, error_hash, ts }`. No file names, no paths, no drive serials.
- Endpoint over TLS, public-key pinning.

### T6 — License key replay across machines
**Likelihood:** medium (commercial concern).
**Impact:** revenue loss.
**Mitigations (post-v0.1):**
- Activation binds to a hash of `(volume_serial, machine_guid, cpu_brand)`.
- Offline keys signed Ed25519; server-issued JWTs for online activation.
- Floating licenses use a local "lease" file with TTL.

### T7 — Update channel compromise
**Likelihood:** low.
**Impact:** RCE for all users.
**Mitigations (post-v0.1):**
- Updater pulls from `updates.trirecover.trimind.tech` over TLS.
- Update artifacts are Ed25519-signed; public key is pinned in the installed binary.
- Updater refuses to apply an update with a lower version than installed (downgrade attack).

## Out of scope

- Defending against a malicious local administrator who edits the binary.
- Defending against attackers with physical access to the running machine.
- Cryptographic recovery of intentionally-encrypted-then-deleted files.
