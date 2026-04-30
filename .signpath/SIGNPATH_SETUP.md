# SignPath Foundation setup

One-time onboarding to get free Authenticode signing for glassbar's
release binaries. After this is done, every `git tag vX.Y.Z && git push`
produces a signed MSI + signed .exes on the GitHub Release page.

## 1. Apply for SignPath Foundation

Go to <https://signpath.io/foundation> → **Apply now**. The application form
asks for:

| Field | What to put |
|---|---|
| Project name | `glassbar` |
| Project URL | `https://github.com/pikammmmm/glassbar` |
| License | `MIT` (the `LICENSE` file at the repo root) |
| Description | "Glassy floating dock + HUD for Windows. Rust + Tauri 2 desktop app." |
| Maintainer name | (your real name — they verify against GitHub) |
| Maintainer email | your email |
| Why you need signing | "Distributing standalone .exe + MSI installer on GitHub Releases. SmartScreen blocks unsigned downloads which is hostile to first-time users." |
| Build pipeline | "GitHub Actions on `windows-latest`, Rust + cargo + tauri-cli." |
| Files to sign | `glassbar.exe`, `uninstall.exe`, `glassbar_*_x64_en-US.msi` |

Approval typically takes **1–2 weeks**. They check that the project is
actually open source, has commits from real maintainers, and that signing
won't be abused.

## 2. After approval — get your three IDs

SignPath emails an invite to your organization on `app.signpath.io`. There:

- **Organization ID** — top-right, copy from the URL or org settings.
- **Project slug** — create a project named `glassbar`, slug shown in the URL.
- **Signing policy slug** — under the project, set up two policies:
  - `release-signing` for tagged builds (full Authenticode)
  - `test-signing` for `workflow_dispatch` runs (optional)

  The workflow YAML uses one variable, `SIGNPATH_SIGNING_POLICY_SLUG`, so
  set it to whichever you want as the default (typically `release-signing`).

## 3. Wire the IDs + token into GitHub

Repo: `pikammmmm/glassbar` → **Settings → Secrets and variables → Actions**.

**Variables** (clear-text, OK to share):

| Name | Value |
|---|---|
| `SIGNPATH_ORGANIZATION_ID` | from step 2 |
| `SIGNPATH_PROJECT_SLUG` | `glassbar` |
| `SIGNPATH_SIGNING_POLICY_SLUG` | e.g. `release-signing` |

**Secrets** (encrypted):

| Name | Value |
|---|---|
| `SIGNPATH_API_TOKEN` | Generate in SignPath → user profile → API tokens → "GitHub Actions" |

## 4. Land the workflow

The workflow YAML lives at `.github/workflows/release.yml` in this repo
already (locally — see step 5 if it isn't on GitHub yet). It builds, ships
the artifacts to SignPath, waits for the signed bundle to come back, and
attaches signed assets to the Release.

## 5. The "workflow scope" gotcha

GitHub OAuth tokens issued via `gh auth login` don't have the `workflow`
scope by default, so pushing files under `.github/workflows/` is rejected.
Two ways to fix:

- **Run once:** `gh auth refresh -s workflow` and confirm in the browser.
  Subsequent `git push` works normally.
- **Web UI:** open `pikammmmm/glassbar` → **Add file → Create new file** →
  paste path `.github/workflows/release.yml` → paste contents from your
  local file → commit. No scope needed.

## 6. Cut the first signed release

```bash
git tag v0.1.1
git push origin v0.1.1
```

The Actions tab shows the build → SignPath sign-request → wait-for-signed
→ release-publish. Total runtime ~5 min on a warm cache.

The first SmartScreen warning may still appear (Authenticode reputation
takes a handful of downloads to build), but the *publisher* line in the
warning will read `pikammmmm` instead of "Unknown publisher", and the
warning disappears entirely once the cumulative download count crosses
SmartScreen's reputation threshold.
