# Old workflows — KEPT FOR REFERENCE, NOT LIVE

These are archived copies. They are deliberately OUTSIDE `.github/workflows/`
and renamed `.disabled`, because **GitHub runs everything in that folder** —
a file called "backup" is still a live workflow.

`release_v241_backup.yml` was an old copy of the release workflow that still
carried `on: push: tags: - "v*"`. It therefore ran on EVERY version tag,
alongside the real release workflow. Two runners each built and SIGNED their
own installer, then raced to upload to the same GitHub Release. One won the
.msi, the other won latest.json — so the published signature belonged to a
binary that was never uploaded, and every auto-update failed with
"signature didn't match". It also produced the harmless-looking
"Not Found - update-a-release-asset" error (the loser failing to overwrite).
