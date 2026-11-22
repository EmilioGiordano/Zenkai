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
- Each secret key has two offline copies, in two different physical places, for example two
  USB drives kept in two different buildings. The passwords are kept apart from the drives.
- Only the current secret key and its password are online, as secrets of the
  `release-signing` GitHub Environment. Never as repository or organization secrets.
- A tag build fails unless `RELEASE_KEYS` holds exactly two distinct keys, neither a TEST key.
- The TEST keys in `crates/app/tests/fixtures/update/make-fixtures.mjs` have public secrets.
  They are only for tests and the `test-keys` feature; never sign a release with them.

## Install minisign

1. Download `minisign-0.12-win64.zip` from https://github.com/jedisct1/minisign/releases/tag/0.12.
2. Check `Get-FileHash minisign-0.12-win64.zip` is
   `37B600344E20C19314B2E82813DB2BFDCC408B77B876F7727889DBD46D539479`.
3. Unpack it outside any synced folder. The release job uses the same version on Linux.

## Generate the keys (once, before v0.1.0)

Plug in the first USB drive (here `E:`). Do not write the keys anywhere else.

```
minisign -G -p E:\zenkai-current.pub -s E:\zenkai-current.key
minisign -G -p E:\zenkai-next.pub -s E:\zenkai-next.key
```

- Each command asks for a password twice. Use a different long password for each key.
- Copy the four files to the second USB drive. Check the copies with `Get-FileHash`.
- Store the drives in their two places and the two passwords apart from them.

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

## Configure the environment

In GitHub, Settings > Environments > New environment, name `release-signing`:

1. Required reviewers: the maintainer. Leave "Prevent self-review" off, since the maintainer
   both pushes the tag and approves the signing.
2. Turn off "Allow administrators to bypass configured protection rules".
3. Deployment branches and tags: "Selected branches and tags", one rule of type Tag with
   pattern `v*`, no branch rule.
4. Add the two environment secrets from PowerShell, with the first USB drive plugged in:

   ```
   Get-Content -Raw E:\zenkai-current.key | gh secret set MINISIGN_SECRET_KEY --env release-signing
   gh secret set MINISIGN_PASSWORD --env release-signing
   ```

   The second command prompts for the password; type it there, never on the command line.

## Cut a release

1. On `main`: the version in `[workspace.package]` of `Cargo.toml` and a `## [X.Y.Z]`
   section in `CHANGELOG.md`.
2. `git tag vX.Y.Z <commit on main>` and `git push origin vX.Y.Z`.
3. In Actions, the Release run waits at "Sign the update zips". Approve it only if the run is
   for the tag just pushed and its commit is the expected one.
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
2. Generate a new pair on the offline drive, as above, as `zenkai-next2`.
3. On a new branch, set `RELEASE_KEYS` to the next public key, then the new one.
4. Put the next secret key and its password in the environment secrets.
5. Cut a release as soon as possible. It is signed with the next key, which every installed
   version trusts, and carries the new pair. Say in its notes that the old key is revoked.
6. From then on next is current and the new pair is next. Rename the offline files and
   update both drives.

Installs that have not updated yet still trust the leaked key, so publish fast. If the next
key leaks instead, do the same while signing with the current key: `RELEASE_KEYS` becomes the
current public key, then a new one.

## Both keys are lost or leaked

- Installed versions can no longer update automatically: no key they trust is available.
- Generate two new pairs and cut a release with them. Users must download and install it by
  hand; say so in the release notes and the README.
- If both leaked, anyone holding them can sign an update that installed versions accept.
  Tell users to reinstall by hand from the Releases page.
