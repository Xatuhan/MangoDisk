# macOS application date evidence

Date capture is read-only presentation evidence. It never changes uninstall eligibility,
cleanup selection, application identity, or inventory revision fingerprints. Catalog scans
refresh dates independently of the cached installation inventory; execution preflight does
not repeat optional date discovery. The catalog wire schema is version 12.

Usage records prefer native Spotlight and local foreground history. `kMDItemUsedDates`
supplies UTC-encoded calendar days only; oversized arrays retain the recent 4,096 entries
with partial provenance. Optional knowledgeC `/app/usage` and Biome `App.InFocus/local`
readers exclude synced devices, validate supported schemas/records, and fail open to the
remaining date sources without changing permissions. Biome accepts only live records in the supported SEGB
layout with valid CRCs. Unsupported formats are partial evidence, never empty proof of
disuse. History associated only by bundle ID is ignored for duplicate installed identities.

Without usage records, exact bundle-ID preference domains and application-owned data
provide estimated activity. Display-name guesses, shared vendor/browser directories,
installer logs, updater domains, and symbolic links are excluded. Protected container reads
require a successful, noninteractive System Shared File List access probe; unknown or denied
access skips those roots rather than risking a kernel TCC wait. Permissions are never changed. Data-file modification
has stronger provenance than cache/log changes; directory modification is the final estimate.
Traversal is cancellable and bounded to 1,024 entries per application, 20,000 per capture,
five descendant levels, and a shared 250 ms traversal budget. Host preference names are
indexed once per capture with a separate 4,096-entry bound. Cheap root observations remain
available after that budget; partial evidence may omit a newer change.

Installation receipts require matching package identity/version and an exact Info.plist
payload path before overriding an added/creation estimate. Receipt discovery and payload
commands are bounded. Added dates may reflect moving or replacing an app; creation dates
may survive an archive or restore. None of these estimates promises first installation.

The desktop adapter observes foreground activation/deactivation only while MangoDisk runs.
Core persists at most 1,024 latest path-scoped observations for 365 days in schema 1
`application-usage.json`, coalesces writes to 30 seconds, and flushes on normal shutdown.
Unsupported or corrupt documents are preserved read-only; no downgrade overwrites them.
Window titles, keystrokes, documents, and configuration contents are not collected.

Run native date tests with `cargo test -p mangodisk-platform application_dates`. The ignored
`actual_host_date_capture_probe` reports aggregate coverage/timing without application paths.
