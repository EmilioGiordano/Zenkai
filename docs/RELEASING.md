# Releasing

The maintainer's procedure for the update signing keys and for cutting a release. The rules
come from `docs/SPEC.md`, section "Actualizaciones", "Firma de las releases". Rules, not prose.

## Rules

- There are two minisign key pairs: **current** and **next**. Both public keys are compiled
  into the app, current first, in `RELEASE_KEYS` in `crates/app/src/updates/signature.rs`.
- Every release zip is signed by the `sign` job of `.github/workflows/release.yml` with the
  current key and the trusted comment `zenkai/<version>/<platform>`. Nothing else signs.
- Both secret keys are encrypted with a password. Never `minisign -G -W`.
- Never put a secret key or its password in the repository, an issue, a PR, a chat, a log, or
  a folder synced to a cloud (OneDrive, Dropbox, iCloud, Google Drive). This includes the
  Desktop and Documents folders when OneDrive backs them up.
- Each secret key has two offline copies on drives of its own: the current pair on drives A1
  and A2, the next pair on drives B1 and B2. The four drives are kept in at least two
  different physical places, and A1 never sits with A2, nor B1 with B2.
- The two passwords are stored apart from each other and apart from every drive.
- Only the current secret key and its password are online, as secrets of the
  `release-signing` GitHub Environment. Never as repository or organization secrets.
- A tag build fails unless `RELEASE_KEYS` holds exactly two distinct keys, neither a TEST key.
- Only plain `vX.Y.Z` tags are signed; a prerelease tag fails the `sign` job.
- The workflow that runs, `sign` job included, is the one in the tagged commit. Only the
  maintainer creates `v*` tags, and the maintainer approves only runs the maintainer started.
- The TEST keys in `crates/app/tests/fixtures/update/make-fixtures.mjs` have public secrets.
  They are only for tests and the `test-keys` feature; never sign a release with them.

## Install minisign

1. Download `minisign-0.12-win64.zip` from https://github.com/jedisct1/minisign/releases/tag/0.12.
2. Check `Get-FileHash minisign-0.12-win64.zip` is
   `37B600344E20C19314B2E82813DB2BFDCC408B77B876F7727889DBD46D539479`.
3. Unpack it outside any synced folder. The release job uses the same version on Linux.

## Generate the keys (once, before v0.1.0)

Write each pair straight to its own drive; never anywhere else.

1. Plug in drive A1 (here `E:`) and run `minisign -G -p E:\zenkai-current.pub -s E:\zenkai-current.key`.
2. Plug in drive B1 (here `F:`) and run `minisign -G -p F:\zenkai-next.pub -s F:\zenkai-next.key`.
3. Each command asks for a password twice. Use a different long password for each key and
   store each one apart from the other and from the drives.
4. Copy A1's two files to A2 and B1's two files to B2. Check the copies with `Get-FileHash`.
5. Keep the four drives as the rules say.

## Put the public keys in the app

1. On a new branch, paste the second line of `zenkai-current.pub`, then the second line of
   `zenkai-next.pub`, into `RELEASE_KEYS`:

   ```rust
   const RELEASE_KEYS: &[&str] = &[
       "RW...current...",
       "RW...next...",
   ];
   ```

2. Run `cargo test -p zenkai signature`: `a_release_build_trusts_only_the_real_pair` must pass.
3. Merge it through a normal PR before tagging.

## Protect the tags

In GitHub, Settings > Rules > Rulesets > New tag ruleset, name `release tags`:

1. Enforcement status: Active.
2. Target tags: include by pattern `v*`.
3. Rules: Restrict creations, Restrict updates, Restrict deletions.
4. Bypass list: only the Repository admin role, which on this repository is the maintainer
   alone. No other role, team, deploy key or app, GitHub Actions included.
5. Keep the maintainer the only admin; adding another admin lets them create release tags.

## Configure the environment

In GitHub, Settings > Environments > New environment, name `release-signing`:

1. Required reviewers: the maintainer. Leave "Prevent self-review" off, since the maintainer
   both pushes the tag and approves the signing.
2. Turn off "Allow administrators to bypass configured protection rules".
3. Deployment branches and tags: "Selected branches and tags", one rule of type Tag with
   pattern `v*`, no branch rule.
4. Add the two environment secrets from PowerShell, with drive A1 plugged in as `E:`:

   ```
   Get-Content -Raw E:\zenkai-current.key | gh secret set MINISIGN_SECRET_KEY --env release-signing
   gh secret set MINISIGN_PASSWORD --env release-signing
   ```

   The second command prompts for the password; type it there, never on the command line.

## Cut a release

1. On `main`: the version in `[workspace.package]` of `Cargo.toml` and a `## [X.Y.Z]`
   section in `CHANGELOG.md`.
2. `git fetch origin`, then `git rev-parse origin/main`, and note that SHA. Tag exactly it:
   `git tag vX.Y.Z <that SHA>` and `git push origin vX.Y.Z`.
3. In Actions, the Release run waits at "Sign the update zips". Before "Review deployments":
   - The run was started by your own tag push, a moment ago. Never approve a run you did
     not start: the workflow comes from the tagged commit, so whoever made the tag chose
     what the signing job does.
   - The run's commit SHA (shown on the run page) equals the SHA noted in step 2.
   - The tag in the run title is `vX.Y.Z`.
   If anything differs, reject the deployment, delete the tag, and find out who pushed it.
4. When the release is published, check the signature from a clean folder:

   ```
   gh release download vX.Y.Z -p "zenkai-vX.Y.Z-windows-x64.zip*"
   minisign -V -P <current public key> -m zenkai-vX.Y.Z-windows-x64.zip
   ```

   It must print `Trusted comment: zenkai/X.Y.Z/windows-x64`.
5. After installing a new minisign version, run
   `python crates/app/tests/fixtures/update/check-minisign-cli.py <path to minisign>`. It
   checks, with the TEST keys only, that the CLI still signs exactly as the updater expects.

## The current key leaks

1. Delete `MINISIGN_SECRET_KEY` and `MINISIGN_PASSWORD` from the environment at once.
2. Find and close the path it leaked through before the next key goes anywhere near GitHub:
   - the environment's deployment history: every approved run, its tag, commit and actor;
   - personal access tokens, deploy keys, GitHub Apps and collaborators with access;
   - the tag ruleset, the environment's reviewers and branch rules, and recent changes to
     `.github/workflows/`;
   - the account's 2FA, sessions and security log.
   If the path cannot be found and closed, treat it as both keys leaked (below).
3. Generate a new pair on two new drives of its own, as above, as `zenkai-next2`.
4. On a new branch, set `RELEASE_KEYS` to the next public key, then the new one.
5. Put the next secret key and its password in the environment secrets.
6. Cut a release as soon as possible. It is signed with the next key, which every installed
   version trusts, and carries the new pair. Say in its notes that the old key is revoked.
7. From then on next is current and the new pair is next. Relabel the drives (B becomes A,
   the new ones B) and wipe the leaked key's drives.

Installs that have not updated yet still trust the leaked key, so publish fast. If the next
key leaks instead, do the same while signing with the current key: `RELEASE_KEYS` becomes the
current public key, then a new one.

## Both keys are lost or leaked

- Installed versions can no longer update automatically: no key they trust is available.
- Generate two new pairs and cut a release with them. Users must download and install it by
  hand; say so in the release notes and the README.
- If both leaked, anyone holding them can sign an update that installed versions accept.
  Tell users to reinstall by hand from the Releases page.
