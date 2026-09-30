English | [日本語](./README.ja.md)

# kukuri

kukuri is a topic-first P2P social app. Find a topic you care about, join a public conversation, or move into a smaller private channel while keeping your account and posts rooted on your own device.

![kukuri desktop preview showing Timeline, Thread, and Profile columns with the Control Center bar](./docs/assets/readme/kukuri-desktop-preview.png)

## Download the Builder Preview

> [!IMPORTANT]
> kukuri is currently a **Builder Preview for testers**, not a stable general release.

**[Download the latest Windows / Linux Preview](https://github.com/kukuri-app/kukuri/releases/latest)**

| Platform | Current support |
| --- | --- |
| Windows 10 / 11 | NSIS installer from the latest GitHub Release |
| Linux | AppImage / Deb (x86_64) and CLI (x86_64, aarch64), published in the same Release as the Windows installer since `v0.2.4-preview.1` (2026-09-15); every Preview Release builds all five packages from one source |
| macOS | No package is currently provided |

Preview installers do not carry an OS code signature, so Windows SmartScreen can show a warning; check the release notes before running the installer. In-app updates are separate: the update manifest is signed and the app rejects a tampered manifest.

For detailed setup and recovery help, see the [user quickstart](./docs/runbooks/mvp-user-quickstart.md) and [troubleshooting guide](./docs/runbooks/mvp-troubleshooting.md). Released changes are listed in the [CHANGELOG](./CHANGELOG.md).

## What You Can Do

- Find and follow topics, then publish posts and replies in threaded conversations.
- Keep public posts and private channels under the same topic instead of splitting the community into separate spaces.
- Follow people, react, repost, quote, bookmark posts, and mute or block users locally. Blocking hides posts in both directions.
- Search and discover posts and topics through the Community Index of a consenting Community Node.
- Exchange direct messages with mutual connections and share images or videos.
- Receive local and operating-system notifications for replies, mentions, follows, reposts, and messages.
- Export your account key as an encrypted passphrase-protected string, import it on another device, and switch between several accounts without restarting.
- Back up one account to a single encrypted file and restore it on another device.
- Use the app in Japanese, English, or Simplified Chinese with a light or dark theme.
- Keep your account, posts, and local state across restarts, temporary offline periods, and Preview updates.

## Try It in 3 Minutes

1. Install and launch the Windows Preview, use a published [Linux AppImage](docs/runbooks/linux-appimage-smoke.md) or [Deb](docs/runbooks/linux-deb.md), or [run from source](#development-quickstart). See [Linux CLI usage](docs/runbooks/linux-cli.md) for the separate CLI profile.
2. On the first screen, pick a language, confirm that you are 18 or older, and accept the app terms and privacy policy. kukuri does not start any network connection until you accept.
3. Read "What is a community node?", choose "Review terms", and accept that node's documents. Connection setup starts only after this explicit consent; choosing "Later" lets you resume from Explore or the Community Node settings.
4. Open the Profile column and set your display name, username, and bio.
5. Open a starter topic (`kukuri:topic:general`, `kukuri:topic:dev`, or `kukuri:topic:test`), publish a public post, and reply to an existing post.
6. Create or join a private channel under the same topic from the "Private channel" button at the top of the Timeline column.
7. Send feedback with the "Send feedback" button in the bottom bar (delivered to a Community Node that accepts tester feedback) or through GitHub. To attach a diagnostic report, enable developer mode in `Settings -> Developer`, then copy or export the report from `Settings -> Release`.

The default diagnostic report omits secret keys, authentication tokens, private-channel secrets, invite/share tokens, direct-message bodies, and local database paths.

## Preview Status and Limits

- Preview targets are Windows NSIS, Linux x86_64 AppImage / Deb, and Linux x86_64/aarch64 CLI. Availability follows the published Release asset list; generation alone is not publication. Deb updates require explicit application and OS authorization; cancellation does not trigger another authentication method. macOS has no package.
- The first launch requires an age self-attestation and acceptance of the app terms; each Community Node additionally requires explicit consent to its own documents before the app uses it. Adult-labeled posts are hidden until you allow them in `Settings -> Safety`.
- A direct message needs another test peer and a mutual relationship. P2P behavior is easiest to evaluate with two devices or two isolated app instances.
- Live, Game, Stream, and Metaverse surfaces are hidden by default and appear only after enabling developer mode in `Settings -> Developer`. Metaverse Domes (Dome hosting, seamless transition, spatial audio, follow camera, connection map, and Dome management) are experimental: their data formats have no backward-compatible decoding or migration guarantee. The Stream surface does not yet include a media player.
- Your account key is stored only on your device. There is no central reissue or recovery; use the encrypted key export or the device backup to keep a copy.
- Preview updates are expected to preserve your account-identifying key, profile, follows, posts, local database state, Iroh data, Community Node settings, private-channel access, and the notification inbox. A device backup does not carry Community Node tokens, consent state, the age attestation, or the adult-content setting; you redo those after a restore. Keep a backup before uninstalling or resetting if you need to retain local state.
- This is testing software. Please attach a sanitized diagnostic report when reporting connectivity, upgrade, or recovery problems.

See the [Builder Preview plan](./docs/progress/2026-04-16-mvp-builder-preview-plan.md) for the current milestone, the [CHANGELOG](./CHANGELOG.md) for what each Preview Release changed, and the [release runbook](./docs/runbooks/release.md) for packaging and data-safety gates.

## How kukuri Works

- **Your account and posts stay with you.** The key that identifies your account is stored locally. A Community Node is not your account owner or home server.
- **P2P is the foundation.** Connectivity prefers direct P2P, then relay-supported P2P, and uses relay fallback only when the earlier paths cannot carry the data.
- **Community Nodes provide scoped assistance.** A node may help with bootstrap, authentication, topic rendezvous, connectivity, indexing, moderation, or reporting. It is not the permanent canonical store for user posts, profiles, or the social graph, and it has no network-wide authority.
- **Different data has different paths.** Structured shared state is synchronized through `docs`, media and large payloads through `blobs`, and `hints` only notify peers that something may need to be synchronized.
- **Nostr compatibility is intentionally limited.** kukuri keeps useful account-key, signed-envelope, and selected tag semantics; it is not a full Nostr client and does not use a relay-first internal sync model.
- **Moderation remains scoped.** A moderation event or safety advisory is optional trust input from its issuing node, not a command applied to the entire network. Each client decides how to use it.

The durable responsibility boundary is documented in [P2P-first Community Node responsibilities](./docs/architecture/p2p-first-community-node-responsibility-boundary.md).

## Available Today

| Area | Current Builder Preview capability |
| --- | --- |
| Topics and posts | Topic discovery, Community Index search with indexing-status display, public posts, replies and threads, reactions, reposts, quotes, bookmarks, and local mute |
| Private conversation | Invite-only, mutual-follow, and mutual-follow-plus channels with epoch-aware membership; pairwise mutual-only direct messages |
| People and activity | Public profiles, follow/unfollow, block with two-way hiding, mutual follows and connections through people you follow, local and OS notifications |
| Media | Image and video attachments in posts and direct messages |
| Safety and account portability | Age self-attestation, adult self-labels hidden by default, encrypted account-key export/import, multi-account switching, and encrypted device backup/restore |
| Localization and appearance | Japanese, English, and Simplified Chinese UI; light and dark themes tuned for WCAG 2.2 AA contrast |
| Connectivity and recovery | Static-peer links, seeded DHT discovery, assistance only from Community Nodes you explicitly consented to, offline-capable local state, restart recovery, and late-join backfill |
| Preview operations | In-app update checks, in-app tester feedback, sanitized diagnostics and an in-app log viewer in developer mode, provenance display, and distributed report routing |
| Experimental (developer mode) | Live sessions, game rooms, the Stream column, and Metaverse Domes |

The [foundation progress record](./docs/progress/2026-03-10-foundation.md) defines the Phase 6 baseline; later changes are tracked in the [CHANGELOG](./CHANGELOG.md), accepted [ADRs](./docs/adr/), tests, and [harness scenarios](./harness/scenarios/).

## Longer-Term Direction

- Optional search, discovery, recommendation, gateway, and bridge services can grow around the P2P core without becoming mandatory canonical stores.
- Community Node trust, moderation, policy-assist, and operator tooling can evolve within each node's declared capability and authority scope.
- Live, game, Stream, and Metaverse experiences are accepted and implemented as experimental features (ADR 0035 to 0045); promoting them into the product scope must not change the topic-first ownership and synchronization boundaries.

These are directions, not a promise that every capability is available in the current Preview.

## Feedback and Community

- Report reproducible bugs and regressions in [GitHub Issues](https://github.com/kukuri-app/kukuri/issues).
- Use [GitHub Discussions](https://github.com/kukuri-app/kukuri/discussions) for questions, product ideas, UX proposals, and early discussion of larger changes.
- The in-app "Send feedback" button delivers tester feedback to a Community Node that opts into receiving it.
- For connectivity, updater, and recovery problems, enable developer mode in `Settings -> Developer` and include the sanitized report from `Settings -> Release`.
- Community Node operators are welcome to report deployment, disclosure, moderation, and distributed-reporting feedback through the same GitHub entry points.

## Contributing

Contributions are welcome in code and beyond it: bug reports, UI/UX proposals, documentation, translations, tests, implementation work, and Community Node operational feedback all help.

For a substantial feature, protocol change, responsibility-boundary change, or large refactor, start a Discussion before implementation. Keep bug reports focused in Issues, and use the repository's tests and documentation as the source of truth for behavior.

### Development Quickstart

Prerequisites:

- Git
- Rust `1.98.1` (pinned by `rust-toolchain.toml`)
- Node.js `^20.19.0` or `>=22.12.0`
- pnpm `10.16.1` through the commands below
- The platform dependencies from the [development runbook](./docs/runbooks/dev.md); Windows development also needs the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/#windows)
- Docker only for Community Node integration tests and local Community Node stacks

```bash
git clone https://github.com/kukuri-app/kukuri.git
cd kukuri

npx pnpm@10.16.1 install --dir apps/desktop
cargo xtask doctor

cd apps/desktop
npx pnpm@10.16.1 tauri:dev
```

Run the normal validation paths from the repository root:

```bash
cargo xtask check
cargo xtask test
cargo xtask e2e-smoke
```

Browser-level UI changes also run `cargo xtask desktop-ui-check`. For browser-only frontend work, use `npx pnpm@10.16.1 --dir apps/desktop dev`. The [development runbook](./docs/runbooks/dev.md) lists targeted checks, UI validation, Community Node workflows, and platform-specific setup.

## Documentation

- [Documentation index](./docs/README.md)
- [CHANGELOG](./CHANGELOG.md)
- [Builder Preview plan](./docs/progress/2026-04-16-mvp-builder-preview-plan.md)
- [Foundation and shipped baseline](./docs/progress/2026-03-10-foundation.md)
- [User quickstart](./docs/runbooks/mvp-user-quickstart.md)
- [Troubleshooting](./docs/runbooks/mvp-troubleshooting.md)
- [Development runbook](./docs/runbooks/dev.md)
- [Release runbook](./docs/runbooks/release.md)
- [P2P-first Community Node responsibility boundary](./docs/architecture/p2p-first-community-node-responsibility-boundary.md)
- [Architecture Decision Records](./docs/adr/)
- [Terms of service](./docs/legal/terms-of-service.md), [privacy policy](./docs/legal/privacy-policy.md), and [external transmission notice](./docs/legal/external-transmission-notice.md)
- [Third-party notices](./docs/THIRD_PARTY_NOTICES.md)

## License

[MIT](./LICENSE)
