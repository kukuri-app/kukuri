# Third-party notices

kukuri preview builds include Rust crates, npm packages, Tauri runtime components, and non-code assets.

This file is generated from the locked root and Tauri Rust workspaces, desktop npm dependency inventories, and docs/ASSET_MANIFEST.json. It includes dependencies across platforms, not only packages linked into one binary. OS libraries and codecs bundled into an AppImage require a separate package-level inventory.

Regenerate it from the repository root with:

```powershell
./scripts/release/generate-third-party-notices.ps1
```

Release owners must review these inventories before publishing a preview build and update the manifest or generator if a package or asset requires additional license or attribution text.

## Current distribution note

Windows preview builds use installer distribution through GitHub Releases. Linux x86_64 AppImage generation and validation are tracked in #889; public release integration remains with #890. A locally validated AppImage does not establish public release readiness. If Windows code signing is not configured, the release notes must state that the preview is unsigned and that SmartScreen warnings are expected.

## Non-code asset notices

The asset manifest records exact repository paths, SHA-256 digests, provenance, and source-only versus bundled-binary scope. The entries below are the distribution-facing license and credit summary.

### Bundled first-party assets

| Asset | Author / rights holder | Source / date | License | Modification | Distribution | Conditions | Credit |
| --- | --- | --- | --- | --- | --- | --- | --- |
| blumochichi VRM avatar | KingYoSun / KingYoSun | created 2026-05-28 | MIT (LICENSE) | None | bundled-binary: apps/desktop/public/blumochichi.vrm | commercial=yes; repository redistribution=yes; binary redistribution=yes | Required: Copyright (c) 2025 KingYoSun |
| kukuri application icon | KingYoSun / KingYoSun | created 2026-03-26 | MIT (LICENSE) | The platform-specific sizes and formats are derived from apps/desktop/app-icon.png. | source-only: apps/desktop/app-icon.png; bundled-binary: apps/desktop/src-tauri/icons/** (52 files) | commercial=yes; repository redistribution=yes; binary redistribution=yes | Required: Copyright (c) 2025 KingYoSun |

### Bundled third-party assets

| Asset | Author / rights holder | Source / date | License | Modification | Distribution | Conditions | Credit |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Idle Loop VRMA | Quaternius / Quaternius | https://quaternius.com/packs/universalanimationlibrary.html (acquired 2026-05-29) | CC0-1.0 (https://creativecommons.org/publicdomain/zero/1.0/) | None | bundled-binary: apps/desktop/public/animation/Idle_Loop.vrma | commercial=yes; repository redistribution=yes; binary redistribution=yes | Not required; Universal Animation Library by Quaternius (courtesy credit) |
| Jump Land VRMA | Quaternius / Quaternius | https://quaternius.com/packs/universalanimationlibrary.html (acquired 2026-05-29) | CC0-1.0 (https://creativecommons.org/publicdomain/zero/1.0/) | None | bundled-binary: apps/desktop/public/animation/Jump_Land.vrma | commercial=yes; repository redistribution=yes; binary redistribution=yes | Not required; Universal Animation Library by Quaternius (courtesy credit) |
| Jump Loop VRMA | Quaternius / Quaternius | https://quaternius.com/packs/universalanimationlibrary.html (acquired 2026-05-29) | CC0-1.0 (https://creativecommons.org/publicdomain/zero/1.0/) | None | bundled-binary: apps/desktop/public/animation/Jump_Loop.vrma | commercial=yes; repository redistribution=yes; binary redistribution=yes | Not required; Universal Animation Library by Quaternius (courtesy credit) |
| Jump Start VRMA | Quaternius / Quaternius | https://quaternius.com/packs/universalanimationlibrary.html (acquired 2026-05-29) | CC0-1.0 (https://creativecommons.org/publicdomain/zero/1.0/) | None | bundled-binary: apps/desktop/public/animation/Jump_Start.vrma | commercial=yes; repository redistribution=yes; binary redistribution=yes | Not required; Universal Animation Library by Quaternius (courtesy credit) |
| Sitting Enter VRMA | Quaternius / Quaternius | https://quaternius.com/packs/universalanimationlibrary.html (acquired 2026-05-29) | CC0-1.0 (https://creativecommons.org/publicdomain/zero/1.0/) | None | bundled-binary: apps/desktop/public/animation/Sitting_Enter.vrma | commercial=yes; repository redistribution=yes; binary redistribution=yes | Not required; Universal Animation Library by Quaternius (courtesy credit) |
| Sitting Exit VRMA | Quaternius / Quaternius | https://quaternius.com/packs/universalanimationlibrary.html (acquired 2026-05-29) | CC0-1.0 (https://creativecommons.org/publicdomain/zero/1.0/) | None | bundled-binary: apps/desktop/public/animation/Sitting_Exit.vrma | commercial=yes; repository redistribution=yes; binary redistribution=yes | Not required; Universal Animation Library by Quaternius (courtesy credit) |
| Sitting Idle Loop VRMA | Quaternius / Quaternius | https://quaternius.com/packs/universalanimationlibrary.html (acquired 2026-05-29) | CC0-1.0 (https://creativecommons.org/publicdomain/zero/1.0/) | None | bundled-binary: apps/desktop/public/animation/Sitting_Idle_Loop.vrma | commercial=yes; repository redistribution=yes; binary redistribution=yes | Not required; Universal Animation Library by Quaternius (courtesy credit) |
| Sitting Talking Loop VRMA | Quaternius / Quaternius | https://quaternius.com/packs/universalanimationlibrary.html (acquired 2026-05-29) | CC0-1.0 (https://creativecommons.org/publicdomain/zero/1.0/) | None | bundled-binary: apps/desktop/public/animation/Sitting_Talking_Loop.vrma | commercial=yes; repository redistribution=yes; binary redistribution=yes | Not required; Universal Animation Library by Quaternius (courtesy credit) |
| Sprint Loop VRMA | Quaternius / Quaternius | https://quaternius.com/packs/universalanimationlibrary.html (acquired 2026-05-29) | CC0-1.0 (https://creativecommons.org/publicdomain/zero/1.0/) | None | bundled-binary: apps/desktop/public/animation/Sprint_Loop.vrma | commercial=yes; repository redistribution=yes; binary redistribution=yes | Not required; Universal Animation Library by Quaternius (courtesy credit) |
| Walk Loop VRMA | Quaternius / Quaternius | https://quaternius.com/packs/universalanimationlibrary.html (acquired 2026-05-29) | CC0-1.0 (https://creativecommons.org/publicdomain/zero/1.0/) | None | bundled-binary: apps/desktop/public/animation/Walk_Loop.vrma | commercial=yes; repository redistribution=yes; binary redistribution=yes | Not required; Universal Animation Library by Quaternius (courtesy credit) |

### Bundled generated or generation-assisted assets

None.

### Source-only non-code assets

None.

## Community Node search dictionary

CN search uses Tantivy 0.26.2 and Lindera 5.3.0 (MIT), with the embedded
`mecab-ipadic-neologd-0.0.7-20200820` dictionary (Apache-2.0 and the IPADIC notice).
The dictionary is downloaded from https://lindera.dev during compilation and
verified against MD5 `3561f0e76980a842dc828b460a8cae96` by Lindera's build script.

IPADIC-NEologd: Copyright (C) 2015-2019 Toshinori Sato (@overlast).
The complete copyright, source acknowledgments, and license notices are in
`ipadic-neologd-COPYING.txt`, `ipadic-NOTICE.txt`, and `Apache-2.0.txt` in this
directory. The dictionary incorporates Hatena keywords, Japan Post postal codes,
Snufkin's station list, Taku Kudo's personal-name entries, and entries extracted
from public web documents; the original acknowledgments and URLs are preserved.
Upstream: https://github.com/neologd/mecab-ipadic-neologd.

Community Node images include these files under `/usr/local/share/kukuri/licenses`.

## Rust crates

Total packages: 1006

| Package | Version | License | Source |
| --- | --- | --- | --- |
| adler2 | 2.0.1 | 0BSD OR MIT OR Apache-2.0 | https://crates.io/crates/adler2 |
| aead | 0.5.2 | MIT OR Apache-2.0 | https://crates.io/crates/aead |
| aead | 0.6.1 | MIT OR Apache-2.0 | https://crates.io/crates/aead |
| aes | 0.8.4 | MIT OR Apache-2.0 | https://crates.io/crates/aes |
| aes | 0.9.3 | MIT OR Apache-2.0 | https://crates.io/crates/aes |
| aes-gcm | 0.10.3 | Apache-2.0 OR MIT | https://crates.io/crates/aes-gcm |
| aho-corasick | 1.1.5 | Unlicense OR MIT | https://crates.io/crates/aho-corasick |
| alloc-no-stdlib | 3.0.0 | BSD-3-Clause | https://crates.io/crates/alloc-no-stdlib |
| alloc-stdlib | 0.3.0 | BSD-3-Clause | https://crates.io/crates/alloc-stdlib |
| allocator-api2 | 0.2.21 | MIT OR Apache-2.0 | https://crates.io/crates/allocator-api2 |
| android_system_properties | 0.1.6 | MIT OR Apache-2.0 | https://crates.io/crates/android_system_properties |
| anstream | 1.0.0 | MIT OR Apache-2.0 | https://crates.io/crates/anstream |
| anstyle | 1.0.14 | MIT OR Apache-2.0 | https://crates.io/crates/anstyle |
| anstyle-parse | 1.0.0 | MIT OR Apache-2.0 | https://crates.io/crates/anstyle-parse |
| anstyle-query | 1.1.5 | MIT OR Apache-2.0 | https://crates.io/crates/anstyle-query |
| anstyle-wincon | 3.0.11 | MIT OR Apache-2.0 | https://crates.io/crates/anstyle-wincon |
| anyhow | 1.0.104 | MIT OR Apache-2.0 | https://crates.io/crates/anyhow |
| apple-native-keyring-store | 1.0.2 | MIT OR Apache-2.0 | https://crates.io/crates/apple-native-keyring-store |
| approx | 0.5.1 | Apache-2.0 | https://crates.io/crates/approx |
| arbitrary | 1.4.2 | MIT OR Apache-2.0 | https://crates.io/crates/arbitrary |
| arc-swap | 1.9.2 | MIT OR Apache-2.0 | https://crates.io/crates/arc-swap |
| arcstr | 1.2.0 | Apache-2.0 OR MIT OR Zlib | https://crates.io/crates/arcstr |
| argon2 | 0.6.0 | MIT OR Apache-2.0 | https://crates.io/crates/argon2 |
| arrayvec | 0.7.8 | MIT OR Apache-2.0 | https://crates.io/crates/arrayvec |
| ashpd | 0.13.13 | MIT | https://crates.io/crates/ashpd |
| asn1-rs | 0.7.2 | MIT OR Apache-2.0 | https://crates.io/crates/asn1-rs |
| asn1-rs-derive | 0.6.0 | MIT OR Apache-2.0 | https://crates.io/crates/asn1-rs-derive |
| asn1-rs-impl | 0.2.0 | MIT/Apache-2.0 | https://crates.io/crates/asn1-rs-impl |
| assert-json-diff | 2.0.2 | MIT | https://crates.io/crates/assert-json-diff |
| async-broadcast | 0.7.2 | MIT OR Apache-2.0 | https://crates.io/crates/async-broadcast |
| async-channel | 2.5.0 | Apache-2.0 OR MIT | https://crates.io/crates/async-channel |
| async-executor | 1.14.0 | Apache-2.0 OR MIT | https://crates.io/crates/async-executor |
| async-io | 2.6.0 | Apache-2.0 OR MIT | https://crates.io/crates/async-io |
| async-lock | 3.4.2 | Apache-2.0 OR MIT | https://crates.io/crates/async-lock |
| async-process | 2.5.0 | Apache-2.0 OR MIT | https://crates.io/crates/async-process |
| async-recursion | 1.1.1 | MIT OR Apache-2.0 | https://crates.io/crates/async-recursion |
| async-signal | 0.2.14 | Apache-2.0 OR MIT | https://crates.io/crates/async-signal |
| async-stream | 0.3.6 | MIT | https://crates.io/crates/async-stream |
| async-stream-impl | 0.3.6 | MIT | https://crates.io/crates/async-stream-impl |
| async-task | 4.7.1 | Apache-2.0 OR MIT | https://crates.io/crates/async-task |
| async-trait | 0.1.92 | MIT OR Apache-2.0 | https://crates.io/crates/async-trait |
| async_io_stream | 0.3.3 | Unlicense | https://crates.io/crates/async_io_stream |
| atk | 0.18.2 | MIT | https://crates.io/crates/atk |
| atk-sys | 0.18.2 | MIT | https://crates.io/crates/atk-sys |
| atoi | 2.0.0 | MIT | https://crates.io/crates/atoi |
| atomic-polyfill | 1.0.3 | MIT OR Apache-2.0 | https://crates.io/crates/atomic-polyfill |
| atomic-waker | 1.1.2 | Apache-2.0 OR MIT | https://crates.io/crates/atomic-waker |
| attohttpc | 0.30.1 | MPL-2.0 | https://crates.io/crates/attohttpc |
| autocfg | 1.5.1 | Apache-2.0 OR MIT | https://crates.io/crates/autocfg |
| aws-lc-rs | 1.18.1 | ISC AND (Apache-2.0 OR ISC) | https://crates.io/crates/aws-lc-rs |
| aws-lc-sys | 0.45.0 | ISC AND (Apache-2.0 OR ISC) AND Apache-2.0 AND MIT AND BSD-3-Clause AND (Apache-2.0 OR ISC OR MIT) AND (Apache-2.0 OR ISC OR MIT-0) | https://crates.io/crates/aws-lc-sys |
| axum | 0.8.9 | MIT | https://crates.io/crates/axum |
| axum-core | 0.5.6 | MIT | https://crates.io/crates/axum-core |
| backon | 1.6.0 | Apache-2.0 | https://crates.io/crates/backon |
| bao-tree | 0.16.1 | MIT OR Apache-2.0 | https://crates.io/crates/bao-tree |
| base16ct | 0.2.0 | Apache-2.0 OR MIT | https://crates.io/crates/base16ct |
| base16ct | 1.0.0 | Apache-2.0 OR MIT | https://crates.io/crates/base16ct |
| base64 | 0.21.7 | MIT OR Apache-2.0 | https://crates.io/crates/base64 |
| base64 | 0.22.1 | MIT OR Apache-2.0 | https://crates.io/crates/base64 |
| base64 | 0.23.1 | MIT OR Apache-2.0 | https://crates.io/crates/base64 |
| base64ct | 1.8.3 | Apache-2.0 OR MIT | https://crates.io/crates/base64ct |
| bech32 | 0.12.0 | MIT | https://crates.io/crates/bech32 |
| binary-merge | 0.1.2 | MIT OR Apache-2.0 | https://crates.io/crates/binary-merge |
| bit-set | 0.8.0 | Apache-2.0 OR MIT | https://crates.io/crates/bit-set |
| bit-vec | 0.8.0 | Apache-2.0 OR MIT | https://crates.io/crates/bit-vec |
| bit-vec | 0.9.1 | Apache-2.0 OR MIT | https://crates.io/crates/bit-vec |
| bitflags | 1.3.2 | MIT/Apache-2.0 | https://crates.io/crates/bitflags |
| bitflags | 2.13.2 | MIT OR Apache-2.0 | https://crates.io/crates/bitflags |
| bitpacking | 0.9.3 | MIT | https://crates.io/crates/bitpacking |
| blake2 | 0.11.0 | MIT OR Apache-2.0 | https://crates.io/crates/blake2 |
| blake3 | 1.8.7 | CC0-1.0 OR Apache-2.0 OR Apache-2.0 WITH LLVM-exception | https://crates.io/crates/blake3 |
| block-buffer | 0.10.4 | MIT OR Apache-2.0 | https://crates.io/crates/block-buffer |
| block-buffer | 0.12.1 | MIT OR Apache-2.0 | https://crates.io/crates/block-buffer |
| block-padding | 0.4.2 | MIT OR Apache-2.0 | https://crates.io/crates/block-padding |
| block2 | 0.6.2 | MIT | https://crates.io/crates/block2 |
| blocking | 1.7.0 | Apache-2.0 OR MIT | https://crates.io/crates/blocking |
| bon | 3.10.2 | MIT OR Apache-2.0 | https://crates.io/crates/bon |
| bon-macros | 3.10.2 | MIT OR Apache-2.0 | https://crates.io/crates/bon-macros |
| brotli | 9.0.0 | BSD-3-Clause AND MIT | https://crates.io/crates/brotli |
| brotli-decompressor | 6.0.1 | BSD-3-Clause/MIT | https://crates.io/crates/brotli-decompressor |
| bs58 | 0.5.1 | MIT/Apache-2.0 | https://crates.io/crates/bs58 |
| bumpalo | 3.20.3 | MIT OR Apache-2.0 | https://crates.io/crates/bumpalo |
| bytecheck | 0.8.3 | MIT | https://crates.io/crates/bytecheck |
| bytecheck_derive | 0.8.3 | MIT | https://crates.io/crates/bytecheck_derive |
| bytemuck | 1.25.2 | Zlib OR Apache-2.0 OR MIT | https://crates.io/crates/bytemuck |
| byteorder | 1.5.0 | Unlicense OR MIT | https://crates.io/crates/byteorder |
| byteorder-lite | 0.1.0 | Unlicense OR MIT | https://crates.io/crates/byteorder-lite |
| bytes | 1.12.1 | MIT | https://crates.io/crates/bytes |
| cairo-rs | 0.18.5 | MIT | https://crates.io/crates/cairo-rs |
| cairo-sys-rs | 0.18.2 | MIT | https://crates.io/crates/cairo-sys-rs |
| camino | 1.2.6 | MIT OR Apache-2.0 | https://crates.io/crates/camino |
| cargo-platform | 0.1.9 | MIT OR Apache-2.0 | https://crates.io/crates/cargo-platform |
| cargo_metadata | 0.19.2 | MIT | https://crates.io/crates/cargo_metadata |
| cargo_toml | 1.0.1 | Apache-2.0 OR MIT | https://crates.io/crates/cargo_toml |
| cast | 0.3.0 | MIT OR Apache-2.0 | https://crates.io/crates/cast |
| cbc | 0.2.1 | MIT OR Apache-2.0 | https://crates.io/crates/cbc |
| cc | 1.5.1 | MIT OR Apache-2.0 | https://crates.io/crates/cc |
| ccm | 0.5.0 | Apache-2.0 OR MIT | https://crates.io/crates/ccm |
| census | 0.4.2 | MIT | https://crates.io/crates/census |
| cesu8 | 1.1.0 | Apache-2.0/MIT | https://crates.io/crates/cesu8 |
| cfb | 0.14.0 | MIT | https://crates.io/crates/cfb |
| cfg-expr | 0.15.8 | MIT OR Apache-2.0 | https://crates.io/crates/cfg-expr |
| cfg-if | 1.0.5 | MIT OR Apache-2.0 | https://crates.io/crates/cfg-if |
| cfg_aliases | 0.2.2 | MIT | https://crates.io/crates/cfg_aliases |
| chacha20 | 0.10.2 | MIT OR Apache-2.0 | https://crates.io/crates/chacha20 |
| chacha20 | 0.9.1 | Apache-2.0 OR MIT | https://crates.io/crates/chacha20 |
| chacha20poly1305 | 0.10.1 | Apache-2.0 OR MIT | https://crates.io/crates/chacha20poly1305 |
| chacha20poly1305 | 0.11.0 | Apache-2.0 OR MIT | https://crates.io/crates/chacha20poly1305 |
| chrono | 0.4.45 | MIT OR Apache-2.0 | https://crates.io/crates/chrono |
| cipher | 0.4.4 | MIT OR Apache-2.0 | https://crates.io/crates/cipher |
| cipher | 0.5.2 | MIT OR Apache-2.0 | https://crates.io/crates/cipher |
| clap | 4.6.7 | MIT OR Apache-2.0 | https://crates.io/crates/clap |
| clap_builder | 4.6.7 | MIT OR Apache-2.0 | https://crates.io/crates/clap_builder |
| clap_derive | 4.6.7 | MIT OR Apache-2.0 | https://crates.io/crates/clap_derive |
| clap_lex | 1.1.1 | MIT OR Apache-2.0 | https://crates.io/crates/clap_lex |
| cmake | 0.1.58 | MIT OR Apache-2.0 | https://crates.io/crates/cmake |
| cmov | 0.5.4 | Apache-2.0 OR MIT | https://crates.io/crates/cmov |
| cobs | 0.3.0 | MIT OR Apache-2.0 | https://crates.io/crates/cobs |
| color_quant | 1.1.0 | MIT | https://crates.io/crates/color_quant |
| colorchoice | 1.0.5 | MIT OR Apache-2.0 | https://crates.io/crates/colorchoice |
| combine | 4.6.8 | MIT | https://crates.io/crates/combine |
| concurrent-queue | 2.5.0 | Apache-2.0 OR MIT | https://crates.io/crates/concurrent-queue |
| const-oid | 0.10.2 | Apache-2.0 OR MIT | https://crates.io/crates/const-oid |
| const-oid | 0.9.6 | Apache-2.0 OR MIT | https://crates.io/crates/const-oid |
| const-random | 0.1.18 | MIT OR Apache-2.0 | https://crates.io/crates/const-random |
| const-random-macro | 0.1.16 | MIT OR Apache-2.0 | https://crates.io/crates/const-random-macro |
| constant_time_eq | 0.4.2 | CC0-1.0 OR MIT-0 OR Apache-2.0 | https://crates.io/crates/constant_time_eq |
| convert_case | 0.10.0 | MIT | https://crates.io/crates/convert_case |
| cookie | 0.18.2 | MIT OR Apache-2.0 | https://crates.io/crates/cookie |
| cordyceps | 0.3.5 | MIT | https://crates.io/crates/cordyceps |
| core-foundation | 0.10.1 | MIT OR Apache-2.0 | https://crates.io/crates/core-foundation |
| core-foundation | 0.9.4 | MIT OR Apache-2.0 | https://crates.io/crates/core-foundation |
| core-foundation-sys | 0.8.7 | MIT OR Apache-2.0 | https://crates.io/crates/core-foundation-sys |
| core-graphics | 0.25.0 | MIT OR Apache-2.0 | https://crates.io/crates/core-graphics |
| core-graphics-types | 0.2.0 | MIT OR Apache-2.0 | https://crates.io/crates/core-graphics-types |
| core_detect | 1.0.0 | MIT/Apache-2.0 | https://crates.io/crates/core_detect |
| cpubits | 0.1.1 | MIT OR Apache-2.0 | https://crates.io/crates/cpubits |
| cpufeatures | 0.2.17 | MIT OR Apache-2.0 | https://crates.io/crates/cpufeatures |
| cpufeatures | 0.3.1 | MIT OR Apache-2.0 | https://crates.io/crates/cpufeatures |
| crawdad | 0.3.0 | MIT OR Apache-2.0 | https://crates.io/crates/crawdad |
| crc | 3.4.0 | MIT OR Apache-2.0 | https://crates.io/crates/crc |
| crc-catalog | 2.5.0 | MIT OR Apache-2.0 | https://crates.io/crates/crc-catalog |
| crc32fast | 1.5.2 | MIT OR Apache-2.0 | https://crates.io/crates/crc32fast |
| critical-section | 1.2.0 | MIT OR Apache-2.0 | https://crates.io/crates/critical-section |
| crossbeam-channel | 0.5.17 | MIT OR Apache-2.0 | https://crates.io/crates/crossbeam-channel |
| crossbeam-deque | 0.8.8 | MIT OR Apache-2.0 | https://crates.io/crates/crossbeam-deque |
| crossbeam-epoch | 0.9.21 | MIT OR Apache-2.0 | https://crates.io/crates/crossbeam-epoch |
| crossbeam-queue | 0.3.14 | MIT OR Apache-2.0 | https://crates.io/crates/crossbeam-queue |
| crossbeam-utils | 0.8.23 | MIT OR Apache-2.0 | https://crates.io/crates/crossbeam-utils |
| crunchy | 0.2.4 | MIT | https://crates.io/crates/crunchy |
| crypto-bigint | 0.5.5 | Apache-2.0 OR MIT | https://crates.io/crates/crypto-bigint |
| crypto-common | 0.1.7 | MIT OR Apache-2.0 | https://crates.io/crates/crypto-common |
| crypto-common | 0.2.2 | MIT OR Apache-2.0 | https://crates.io/crates/crypto-common |
| cssparser | 0.37.0 | MPL-2.0 | https://crates.io/crates/cssparser |
| cssparser-macros | 0.7.1 | MPL-2.0 | https://crates.io/crates/cssparser-macros |
| csv | 1.4.0 | Unlicense/MIT | https://crates.io/crates/csv |
| csv-core | 0.1.13 | Unlicense/MIT | https://crates.io/crates/csv-core |
| ctor | 1.0.13 | Apache-2.0 OR MIT | https://crates.io/crates/ctor |
| ctr | 0.9.2 | MIT OR Apache-2.0 | https://crates.io/crates/ctr |
| ctutils | 0.4.2 | Apache-2.0 OR MIT | https://crates.io/crates/ctutils |
| curve25519-dalek | 4.1.3 | BSD-3-Clause | https://crates.io/crates/curve25519-dalek |
| curve25519-dalek | 5.0.0 | BSD-3-Clause | https://crates.io/crates/curve25519-dalek |
| curve25519-dalek-derive | 0.1.1 | MIT/Apache-2.0 | https://crates.io/crates/curve25519-dalek-derive |
| daachorse | 4.0.0 | MIT OR Apache-2.0 | https://crates.io/crates/daachorse |
| darling | 0.24.1 | MIT | https://crates.io/crates/darling |
| darling_core | 0.24.1 | MIT | https://crates.io/crates/darling_core |
| darling_macro | 0.24.1 | MIT | https://crates.io/crates/darling_macro |
| dashmap | 6.2.1 | MIT | https://crates.io/crates/dashmap |
| data-encoding | 2.11.1 | MIT | https://crates.io/crates/data-encoding |
| data-encoding-macro | 0.1.21 | MIT | https://crates.io/crates/data-encoding-macro |
| data-encoding-macro-internal | 0.1.19 | MIT | https://crates.io/crates/data-encoding-macro-internal |
| datasketches | 0.2.0 | Apache-2.0 | https://crates.io/crates/datasketches |
| dbus | 0.9.12 | Apache-2.0/MIT | https://crates.io/crates/dbus |
| deadpool | 0.12.3 | MIT OR Apache-2.0 | https://crates.io/crates/deadpool |
| deadpool-runtime | 0.1.4 | MIT OR Apache-2.0 | https://crates.io/crates/deadpool-runtime |
| defmt | 1.1.1 | MIT OR Apache-2.0 | https://crates.io/crates/defmt |
| defmt-macros | 1.1.1 | MIT OR Apache-2.0 | https://crates.io/crates/defmt-macros |
| defmt-parser | 1.0.0 | MIT OR Apache-2.0 | https://crates.io/crates/defmt-parser |
| der | 0.7.10 | Apache-2.0 OR MIT | https://crates.io/crates/der |
| der | 0.8.2 | Apache-2.0 OR MIT | https://crates.io/crates/der |
| der-parser | 10.0.0 | MIT OR Apache-2.0 | https://crates.io/crates/der-parser |
| der_derive | 0.7.3 | Apache-2.0 OR MIT | https://crates.io/crates/der_derive |
| deranged | 0.5.8 | MIT OR Apache-2.0 | https://crates.io/crates/deranged |
| derive_arbitrary | 1.4.2 | MIT OR Apache-2.0 | https://crates.io/crates/derive_arbitrary |
| derive_more | 2.1.1 | MIT | https://crates.io/crates/derive_more |
| derive_more-impl | 2.1.1 | MIT | https://crates.io/crates/derive_more-impl |
| diatomic-waker | 0.2.3 | MIT OR Apache-2.0 | https://crates.io/crates/diatomic-waker |
| digest | 0.10.7 | MIT OR Apache-2.0 | https://crates.io/crates/digest |
| digest | 0.11.3 | MIT OR Apache-2.0 | https://crates.io/crates/digest |
| dimpl | 0.7.4 | MIT OR Apache-2.0 | https://crates.io/crates/dimpl |
| dirs | 7.0.0 | MIT OR Apache-2.0 | https://crates.io/crates/dirs |
| dirs-sys | 0.5.0 | MIT OR Apache-2.0 | https://crates.io/crates/dirs-sys |
| dispatch2 | 0.3.1 | Zlib OR Apache-2.0 OR MIT | https://crates.io/crates/dispatch2 |
| displaydoc | 0.2.7 | MIT OR Apache-2.0 | https://crates.io/crates/displaydoc |
| dlopen2 | 0.8.2 | MIT | https://crates.io/crates/dlopen2 |
| dlopen2_derive | 0.4.3 | MIT | https://crates.io/crates/dlopen2_derive |
| dlv-list | 0.5.2 | MIT OR Apache-2.0 | https://crates.io/crates/dlv-list |
| dom_query | 0.28.0 | MIT | https://crates.io/crates/dom_query |
| dotenvy | 0.15.7 | MIT | https://crates.io/crates/dotenvy |
| downcast-rs | 2.0.2 | MIT OR Apache-2.0 | https://crates.io/crates/downcast-rs |
| dpi | 0.1.2 | Apache-2.0 AND MIT | https://crates.io/crates/dpi |
| dtoa | 1.0.11 | MIT OR Apache-2.0 | https://crates.io/crates/dtoa |
| dtoa-short | 0.3.5 | MPL-2.0 | https://crates.io/crates/dtoa-short |
| dunce | 1.0.5 | CC0-1.0 OR MIT-0 OR Apache-2.0 | https://crates.io/crates/dunce |
| dyn-clone | 1.0.20 | MIT OR Apache-2.0 | https://crates.io/crates/dyn-clone |
| ecdsa | 0.16.9 | Apache-2.0 OR MIT | https://crates.io/crates/ecdsa |
| ed25519 | 3.0.0 | Apache-2.0 OR MIT | https://crates.io/crates/ed25519 |
| ed25519-dalek | 3.0.0 | BSD-3-Clause | https://crates.io/crates/ed25519-dalek |
| either | 1.18.0 | MIT OR Apache-2.0 | https://crates.io/crates/either |
| elliptic-curve | 0.13.8 | Apache-2.0 OR MIT | https://crates.io/crates/elliptic-curve |
| embed-resource | 3.0.11 | MIT | https://crates.io/crates/embed-resource |
| embed_plist | 1.2.2 | MIT OR Apache-2.0 | https://crates.io/crates/embed_plist |
| embedded-io | 0.4.0 | MIT OR Apache-2.0 | https://crates.io/crates/embedded-io |
| embedded-io | 0.6.1 | MIT OR Apache-2.0 | https://crates.io/crates/embedded-io |
| ena | 0.14.4 | MIT OR Apache-2.0 | https://crates.io/crates/ena |
| encoding_rs | 0.8.42 | (Apache-2.0 OR MIT) AND BSD-3-Clause | https://crates.io/crates/encoding_rs |
| encoding_rs_io | 0.1.8 | MIT OR Apache-2.0 | https://crates.io/crates/encoding_rs_io |
| endi | 1.1.1 | MIT | https://crates.io/crates/endi |
| enum-assoc | 1.4.1 | MIT OR Apache-2.0 | https://crates.io/crates/enum-assoc |
| enumflags2 | 0.7.12 | MIT OR Apache-2.0 | https://crates.io/crates/enumflags2 |
| enumflags2_derive | 0.7.12 | MIT OR Apache-2.0 | https://crates.io/crates/enumflags2_derive |
| equivalent | 1.0.2 | Apache-2.0 OR MIT | https://crates.io/crates/equivalent |
| erased-serde | 0.4.10 | MIT OR Apache-2.0 | https://crates.io/crates/erased-serde |
| errno | 0.3.14 | MIT OR Apache-2.0 | https://crates.io/crates/errno |
| etcetera | 0.11.0 | MIT OR Apache-2.0 | https://crates.io/crates/etcetera |
| event-listener | 5.4.2 | Apache-2.0 OR MIT | https://crates.io/crates/event-listener |
| event-listener-strategy | 0.5.4 | Apache-2.0 OR MIT | https://crates.io/crates/event-listener-strategy |
| fastbloom | 0.17.0 | MIT OR Apache-2.0 | https://crates.io/crates/fastbloom |
| fastdivide | 0.4.2 | zlib-acknowledgement OR MIT | https://crates.io/crates/fastdivide |
| fastrand | 2.5.0 | Apache-2.0 OR MIT | https://crates.io/crates/fastrand |
| fdeflate | 0.3.7 | MIT OR Apache-2.0 | https://crates.io/crates/fdeflate |
| ff | 0.13.1 | MIT/Apache-2.0 | https://crates.io/crates/ff |
| fiat-crypto | 0.2.9 | MIT OR Apache-2.0 OR BSD-1-Clause | https://crates.io/crates/fiat-crypto |
| fiat-crypto | 0.3.0 | MIT OR Apache-2.0 OR BSD-1-Clause | https://crates.io/crates/fiat-crypto |
| field-offset | 0.3.6 | MIT OR Apache-2.0 | https://crates.io/crates/field-offset |
| filetime | 0.2.29 | MIT/Apache-2.0 | https://crates.io/crates/filetime |
| find-msvc-tools | 0.1.14 | MIT OR Apache-2.0 | https://crates.io/crates/find-msvc-tools |
| fixedbitset | 0.5.7 | MIT OR Apache-2.0 | https://crates.io/crates/fixedbitset |
| flagset | 0.4.7 | Apache-2.0 | https://crates.io/crates/flagset |
| flate2 | 1.1.10 | MIT OR Apache-2.0 | https://crates.io/crates/flate2 |
| flume | 0.12.0 | Apache-2.0/MIT | https://crates.io/crates/flume |
| fnv | 1.0.7 | Apache-2.0 / MIT | https://crates.io/crates/fnv |
| foldhash | 0.2.0 | Zlib | https://crates.io/crates/foldhash |
| foreign-types | 0.5.0 | MIT/Apache-2.0 | https://crates.io/crates/foreign-types |
| foreign-types-macros | 0.2.4 | MIT/Apache-2.0 | https://crates.io/crates/foreign-types-macros |
| foreign-types-shared | 0.3.1 | MIT/Apache-2.0 | https://crates.io/crates/foreign-types-shared |
| form_urlencoded | 1.2.2 | MIT OR Apache-2.0 | https://crates.io/crates/form_urlencoded |
| forwarded-header-value | 0.1.1 | ISC | https://crates.io/crates/forwarded-header-value |
| fs4 | 0.13.1 | MIT OR Apache-2.0 | https://crates.io/crates/fs4 |
| fs_extra | 1.3.0 | MIT | https://crates.io/crates/fs_extra |
| futures | 0.3.34 | MIT OR Apache-2.0 | https://crates.io/crates/futures |
| futures-buffered | 0.2.13 | MIT | https://crates.io/crates/futures-buffered |
| futures-channel | 0.3.34 | MIT OR Apache-2.0 | https://crates.io/crates/futures-channel |
| futures-concurrency | 7.7.1 | MIT OR Apache-2.0 | https://crates.io/crates/futures-concurrency |
| futures-core | 0.3.34 | MIT OR Apache-2.0 | https://crates.io/crates/futures-core |
| futures-executor | 0.3.34 | MIT OR Apache-2.0 | https://crates.io/crates/futures-executor |
| futures-intrusive | 0.5.0 | MIT OR Apache-2.0 | https://crates.io/crates/futures-intrusive |
| futures-io | 0.3.34 | MIT OR Apache-2.0 | https://crates.io/crates/futures-io |
| futures-lite | 2.6.1 | Apache-2.0 OR MIT | https://crates.io/crates/futures-lite |
| futures-macro | 0.3.34 | MIT OR Apache-2.0 | https://crates.io/crates/futures-macro |
| futures-sink | 0.3.34 | MIT OR Apache-2.0 | https://crates.io/crates/futures-sink |
| futures-task | 0.3.34 | MIT OR Apache-2.0 | https://crates.io/crates/futures-task |
| futures-timer | 3.0.4 | MIT/Apache-2.0 | https://crates.io/crates/futures-timer |
| futures-util | 0.3.34 | MIT OR Apache-2.0 | https://crates.io/crates/futures-util |
| gdk | 0.18.2 | MIT | https://crates.io/crates/gdk |
| gdk-pixbuf | 0.18.5 | MIT | https://crates.io/crates/gdk-pixbuf |
| gdk-pixbuf-sys | 0.18.0 | MIT | https://crates.io/crates/gdk-pixbuf-sys |
| gdk-sys | 0.18.2 | MIT | https://crates.io/crates/gdk-sys |
| gdkwayland-sys | 0.18.2 | MIT | https://crates.io/crates/gdkwayland-sys |
| gdkx11 | 0.18.2 | MIT | https://crates.io/crates/gdkx11 |
| gdkx11-sys | 0.18.2 | MIT | https://crates.io/crates/gdkx11-sys |
| genawaiter | 0.99.1 | MIT | https://crates.io/crates/genawaiter |
| genawaiter-macro | 0.99.1 | MIT/Apache-2.0 | https://crates.io/crates/genawaiter-macro |
| genawaiter-proc-macro | 0.99.1 | MIT/Apache-2.0 | https://crates.io/crates/genawaiter-proc-macro |
| generator | 0.8.10 | MIT/Apache-2.0 | https://crates.io/crates/generator |
| generic-array | 0.14.7 | MIT | https://crates.io/crates/generic-array |
| getrandom | 0.2.17 | MIT OR Apache-2.0 | https://crates.io/crates/getrandom |
| getrandom | 0.3.4 | MIT OR Apache-2.0 | https://crates.io/crates/getrandom |
| getrandom | 0.4.3 | MIT OR Apache-2.0 | https://crates.io/crates/getrandom |
| ghash | 0.5.1 | Apache-2.0 OR MIT | https://crates.io/crates/ghash |
| gif | 0.14.2 | MIT OR Apache-2.0 | https://crates.io/crates/gif |
| gio | 0.18.4 | MIT | https://crates.io/crates/gio |
| gio-sys | 0.18.1 | MIT | https://crates.io/crates/gio-sys |
| glam | 0.30.10 | MIT OR Apache-2.0 | https://crates.io/crates/glam |
| glam | 0.31.1 | MIT OR Apache-2.0 | https://crates.io/crates/glam |
| glam | 0.32.1 | MIT OR Apache-2.0 | https://crates.io/crates/glam |
| glam | 0.33.11 | MIT OR Apache-2.0 | https://crates.io/crates/glam |
| glamx | 0.3.1 | MIT OR Apache-2.0 | https://crates.io/crates/glamx |
| glib | 0.18.5 | MIT | https://crates.io/crates/glib |
| glib-macros | 0.18.5 | MIT | https://crates.io/crates/glib-macros |
| glib-sys | 0.18.1 | MIT | https://crates.io/crates/glib-sys |
| glob | 0.3.4 | MIT OR Apache-2.0 | https://crates.io/crates/glob |
| gloo-timers | 0.3.0 | MIT OR Apache-2.0 | https://crates.io/crates/gloo-timers |
| gobject-sys | 0.18.0 | MIT | https://crates.io/crates/gobject-sys |
| governor | 0.10.4 | MIT | https://crates.io/crates/governor |
| group | 0.13.0 | MIT/Apache-2.0 | https://crates.io/crates/group |
| gtk | 0.18.2 | MIT | https://crates.io/crates/gtk |
| gtk-sys | 0.18.2 | MIT | https://crates.io/crates/gtk-sys |
| gtk3-macros | 0.18.2 | MIT | https://crates.io/crates/gtk3-macros |
| h2 | 0.4.19 | MIT | https://crates.io/crates/h2 |
| hash32 | 0.2.1 | MIT OR Apache-2.0 | https://crates.io/crates/hash32 |
| hash32 | 0.3.1 | MIT OR Apache-2.0 | https://crates.io/crates/hash32 |
| hashbrown | 0.12.3 | MIT OR Apache-2.0 | https://crates.io/crates/hashbrown |
| hashbrown | 0.14.5 | MIT OR Apache-2.0 | https://crates.io/crates/hashbrown |
| hashbrown | 0.16.1 | MIT OR Apache-2.0 | https://crates.io/crates/hashbrown |
| hashbrown | 0.17.1 | MIT OR Apache-2.0 | https://crates.io/crates/hashbrown |
| hashlink | 0.11.1 | MIT OR Apache-2.0 | https://crates.io/crates/hashlink |
| heapless | 0.7.17 | MIT OR Apache-2.0 | https://crates.io/crates/heapless |
| heapless | 0.8.0 | MIT OR Apache-2.0 | https://crates.io/crates/heapless |
| heck | 0.4.1 | MIT OR Apache-2.0 | https://crates.io/crates/heck |
| heck | 0.5.0 | MIT OR Apache-2.0 | https://crates.io/crates/heck |
| hermit-abi | 0.5.3 | MIT OR Apache-2.0 | https://crates.io/crates/hermit-abi |
| hex | 0.4.3 | MIT OR Apache-2.0 | https://crates.io/crates/hex |
| hkdf | 0.12.4 | MIT OR Apache-2.0 | https://crates.io/crates/hkdf |
| hkdf | 0.13.0 | MIT OR Apache-2.0 | https://crates.io/crates/hkdf |
| hmac | 0.12.1 | MIT OR Apache-2.0 | https://crates.io/crates/hmac |
| hmac | 0.13.0 | MIT OR Apache-2.0 | https://crates.io/crates/hmac |
| html5ever | 0.39.0 | MIT OR Apache-2.0 | https://crates.io/crates/html5ever |
| html5ever | 0.40.1 | MIT OR Apache-2.0 | https://crates.io/crates/html5ever |
| htmlescape | 0.3.1 | Apache-2.0 / MIT / MPL-2.0 | https://crates.io/crates/htmlescape |
| http | 1.5.0 | MIT OR Apache-2.0 | https://crates.io/crates/http |
| http-body | 1.1.0 | MIT | https://crates.io/crates/http-body |
| http-body-util | 0.1.5 | MIT | https://crates.io/crates/http-body-util |
| http-range | 0.1.5 | MIT | https://crates.io/crates/http-range |
| http-range-header | 0.4.2 | MIT | https://crates.io/crates/http-range-header |
| httparse | 1.10.1 | MIT OR Apache-2.0 | https://crates.io/crates/httparse |
| httpdate | 1.0.3 | MIT OR Apache-2.0 | https://crates.io/crates/httpdate |
| hybrid-array | 0.4.15 | MIT OR Apache-2.0 | https://crates.io/crates/hybrid-array |
| hyper | 1.11.1 | MIT | https://crates.io/crates/hyper |
| hyper-rustls | 0.27.10 | Apache-2.0 OR ISC OR MIT | https://crates.io/crates/hyper-rustls |
| hyper-timeout | 0.5.2 | MIT OR Apache-2.0 | https://crates.io/crates/hyper-timeout |
| hyper-util | 0.1.21 | MIT | https://crates.io/crates/hyper-util |
| iana-time-zone | 0.1.65 | MIT OR Apache-2.0 | https://crates.io/crates/iana-time-zone |
| iana-time-zone-haiku | 0.1.2 | MIT OR Apache-2.0 | https://crates.io/crates/iana-time-zone-haiku |
| ico | 0.5.0 | MIT | https://crates.io/crates/ico |
| icu_collections | 2.3.0 | Unicode-3.0 | https://crates.io/crates/icu_collections |
| icu_locale_core | 2.3.0 | Unicode-3.0 | https://crates.io/crates/icu_locale_core |
| icu_normalizer | 2.3.0 | Unicode-3.0 | https://crates.io/crates/icu_normalizer |
| icu_normalizer_data | 2.3.0 | Unicode-3.0 | https://crates.io/crates/icu_normalizer_data |
| icu_properties | 2.3.0 | Unicode-3.0 | https://crates.io/crates/icu_properties |
| icu_properties_data | 2.3.0 | Unicode-3.0 | https://crates.io/crates/icu_properties_data |
| icu_provider | 2.3.1 | Unicode-3.0 | https://crates.io/crates/icu_provider |
| ident_case | 1.0.1 | MIT/Apache-2.0 | https://crates.io/crates/ident_case |
| identity-hash | 0.1.0 | Apache-2.0 OR MIT | https://crates.io/crates/identity-hash |
| idna | 1.1.0 | MIT OR Apache-2.0 | https://crates.io/crates/idna |
| idna_adapter | 1.2.2 | Apache-2.0 OR MIT | https://crates.io/crates/idna_adapter |
| igd-next | 0.17.1 | MIT | https://crates.io/crates/igd-next |
| image | 0.25.10 | MIT OR Apache-2.0 | https://crates.io/crates/image |
| image-webp | 0.2.4 | MIT OR Apache-2.0 | https://crates.io/crates/image-webp |
| indexmap | 1.9.3 | Apache-2.0 OR MIT | https://crates.io/crates/indexmap |
| indexmap | 2.14.2 | Apache-2.0 OR MIT | https://crates.io/crates/indexmap |
| infer | 0.22.0 | MIT | https://crates.io/crates/infer |
| inout | 0.1.4 | MIT OR Apache-2.0 | https://crates.io/crates/inout |
| inout | 0.2.2 | MIT OR Apache-2.0 | https://crates.io/crates/inout |
| inplace-vec-builder | 0.1.1 | MIT OR Apache-2.0 | https://crates.io/crates/inplace-vec-builder |
| inventory | 0.3.25 | MIT OR Apache-2.0 | https://crates.io/crates/inventory |
| ipconfig | 0.3.4 | MIT/Apache-2.0 | https://crates.io/crates/ipconfig |
| ipnet | 2.12.2 | MIT OR Apache-2.0 | https://crates.io/crates/ipnet |
| iroh | 1.3.0 | MIT OR Apache-2.0 | https://crates.io/crates/iroh |
| iroh-base | 1.3.0 | MIT OR Apache-2.0 | https://crates.io/crates/iroh-base |
| iroh-blobs | 0.103.0 | MIT OR Apache-2.0 | https://crates.io/crates/iroh-blobs |
| iroh-dns | 1.3.0 | MIT OR Apache-2.0 | https://crates.io/crates/iroh-dns |
| iroh-docs | 0.101.0 | MIT/Apache-2.0 | https://crates.io/crates/iroh-docs |
| iroh-gossip | 0.101.0 | MIT/Apache-2.0 | https://crates.io/crates/iroh-gossip |
| iroh-io | 0.6.2 | Apache-2.0 OR MIT | https://crates.io/crates/iroh-io |
| iroh-mainline-address-lookup | 0.6.0 | MIT OR Apache-2.0 | https://crates.io/crates/iroh-mainline-address-lookup |
| iroh-mainline-endpoint-discovery | 0.1.3 | MIT OR Apache-2.0 | https://crates.io/crates/iroh-mainline-endpoint-discovery |
| iroh-metrics | 1.0.2 | MIT OR Apache-2.0 | https://crates.io/crates/iroh-metrics |
| iroh-metrics-derive | 1.0.1 | MIT OR Apache-2.0 | https://crates.io/crates/iroh-metrics-derive |
| iroh-relay | 1.3.0 | MIT OR Apache-2.0 | https://crates.io/crates/iroh-relay |
| iroh-tickets | 1.0.0 | MIT OR Apache-2.0 | https://crates.io/crates/iroh-tickets |
| iroh-util | 0.6.0 | MIT OR Apache-2.0 | https://crates.io/crates/iroh-util |
| irpc | 0.17.0 | Apache-2.0/MIT | https://crates.io/crates/irpc |
| irpc-derive | 0.17.0 | Apache-2.0/MIT | https://crates.io/crates/irpc-derive |
| is | 0.11.1 | MIT OR Apache-2.0 | https://crates.io/crates/is |
| is-docker | 0.2.0 | MIT | https://crates.io/crates/is-docker |
| is-wsl | 0.4.0 | MIT | https://crates.io/crates/is-wsl |
| is_terminal_polyfill | 1.70.2 | MIT OR Apache-2.0 | https://crates.io/crates/is_terminal_polyfill |
| itertools | 0.14.0 | MIT OR Apache-2.0 | https://crates.io/crates/itertools |
| itoa | 1.0.18 | MIT OR Apache-2.0 | https://crates.io/crates/itoa |
| javascriptcore-rs | 1.1.2 | MIT | https://crates.io/crates/javascriptcore-rs |
| javascriptcore-rs-sys | 1.1.1 | MIT | https://crates.io/crates/javascriptcore-rs-sys |
| jiff | 0.2.37 | Unlicense OR MIT | https://crates.io/crates/jiff |
| jiff-core | 0.1.1 | Unlicense OR MIT | https://crates.io/crates/jiff-core |
| jiff-static | 0.2.37 | Unlicense OR MIT | https://crates.io/crates/jiff-static |
| jiff-tzdb | 0.1.8 | Unlicense OR MIT | https://crates.io/crates/jiff-tzdb |
| jiff-tzdb-platform | 0.1.3 | Unlicense OR MIT | https://crates.io/crates/jiff-tzdb-platform |
| jni | 0.21.1 | MIT/Apache-2.0 | https://crates.io/crates/jni |
| jni | 0.22.4 | MIT OR Apache-2.0 | https://crates.io/crates/jni |
| jni-macros | 0.22.4 | MIT OR Apache-2.0 | https://crates.io/crates/jni-macros |
| jni-sys | 0.3.1 | MIT OR Apache-2.0 | https://crates.io/crates/jni-sys |
| jni-sys | 0.4.1 | MIT OR Apache-2.0 | https://crates.io/crates/jni-sys |
| jni-sys-macros | 0.4.1 | MIT OR Apache-2.0 | https://crates.io/crates/jni-sys-macros |
| jobserver | 0.1.35 | MIT OR Apache-2.0 | https://crates.io/crates/jobserver |
| js-sys | 0.3.106 | MIT OR Apache-2.0 | https://crates.io/crates/js-sys |
| json-patch | 4.2.0 | MIT/Apache-2.0 | https://crates.io/crates/json-patch |
| jsonptr | 0.7.1 | MIT OR Apache-2.0 | https://crates.io/crates/jsonptr |
| jsonwebtoken | 11.1.0 | MIT | https://crates.io/crates/jsonwebtoken |
| kanaria | 0.2.0 | MIT | https://crates.io/crates/kanaria |
| keyboard-types | 0.8.3 | MIT OR Apache-2.0 | https://crates.io/crates/keyboard-types |
| keyring | 4.2.0 | MIT OR Apache-2.0 | https://crates.io/crates/keyring |
| keyring-core | 1.0.0 | MIT OR Apache-2.0 | https://crates.io/crates/keyring-core |
| lazy_static | 1.5.0 | MIT OR Apache-2.0 | https://crates.io/crates/lazy_static |
| levenshtein_automata | 0.2.1 | MIT | https://crates.io/crates/levenshtein_automata |
| libappindicator | 0.9.0 | Apache-2.0 OR MIT | https://crates.io/crates/libappindicator |
| libappindicator-sys | 0.9.0 | Apache-2.0 OR MIT | https://crates.io/crates/libappindicator-sys |
| libc | 0.2.189 | MIT OR Apache-2.0 | https://crates.io/crates/libc |
| libdbus-sys | 0.2.7 | Apache-2.0/MIT | https://crates.io/crates/libdbus-sys |
| libloading | 0.7.4 | ISC | https://crates.io/crates/libloading |
| libm | 0.2.16 | MIT | https://crates.io/crates/libm |
| libredox | 0.1.25 | MIT | https://crates.io/crates/libredox |
| libsqlite3-sys | 0.37.0 | MIT | https://crates.io/crates/libsqlite3-sys |
| lindera | 5.3.0 | MIT | https://crates.io/crates/lindera |
| lindera-analysis | 5.3.0 | MIT | https://crates.io/crates/lindera-analysis |
| lindera-dictionary | 5.3.0 | MIT | https://crates.io/crates/lindera-dictionary |
| lindera-ipadic-neologd | 5.3.0 | MIT | https://crates.io/crates/lindera-ipadic-neologd |
| lindera-tantivy | 5.0.1 | MIT | https://crates.io/crates/lindera-tantivy |
| linux-raw-sys | 0.12.1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | https://crates.io/crates/linux-raw-sys |
| litemap | 0.8.3 | Unicode-3.0 | https://crates.io/crates/litemap |
| lock_api | 0.4.14 | MIT OR Apache-2.0 | https://crates.io/crates/lock_api |
| log | 0.4.34 | MIT OR Apache-2.0 | https://crates.io/crates/log |
| loom | 0.7.2 | MIT | https://crates.io/crates/loom |
| lru | 0.16.4 | MIT | https://crates.io/crates/lru |
| lru | 0.18.5 | MIT | https://crates.io/crates/lru |
| lru-slab | 0.1.3 | MIT OR Apache-2.0 OR Zlib | https://crates.io/crates/lru-slab |
| lz4_flex | 0.13.1 | MIT | https://crates.io/crates/lz4_flex |
| mac-addr | 0.3.0 | MIT | https://crates.io/crates/mac-addr |
| mac-notification-sys | 0.6.15 | MIT/Apache-2.0 | https://crates.io/crates/mac-notification-sys |
| markup5ever | 0.39.0 | MIT OR Apache-2.0 | https://crates.io/crates/markup5ever |
| markup5ever | 0.40.0 | MIT OR Apache-2.0 | https://crates.io/crates/markup5ever |
| matchers | 0.2.0 | MIT | https://crates.io/crates/matchers |
| matchit | 0.8.4 | MIT AND BSD-3-Clause | https://crates.io/crates/matchit |
| matrixmultiply | 0.3.11 | MIT/Apache-2.0 | https://crates.io/crates/matrixmultiply |
| md-5 | 0.11.0 | MIT OR Apache-2.0 | https://crates.io/crates/md-5 |
| md5 | 0.8.1 | Apache-2.0 OR MIT | https://crates.io/crates/md5 |
| measure_time | 0.9.0 | MIT | https://crates.io/crates/measure_time |
| memchr | 2.8.3 | Unlicense OR MIT | https://crates.io/crates/memchr |
| memmap2 | 0.9.11 | MIT OR Apache-2.0 | https://crates.io/crates/memmap2 |
| memoffset | 0.9.1 | MIT | https://crates.io/crates/memoffset |
| mime | 0.3.17 | MIT OR Apache-2.0 | https://crates.io/crates/mime |
| mime_guess | 2.0.5 | MIT | https://crates.io/crates/mime_guess |
| minicov | 0.3.8 | Apache-2.0/MIT | https://crates.io/crates/minicov |
| minimal-lexical | 0.2.1 | MIT/Apache-2.0 | https://crates.io/crates/minimal-lexical |
| minisign-verify | 0.2.5 | MIT | https://crates.io/crates/minisign-verify |
| minisign-verify | 0.3.0 | MIT | https://crates.io/crates/minisign-verify |
| miniz_oxide | 0.8.9 | MIT OR Zlib OR Apache-2.0 | https://crates.io/crates/miniz_oxide |
| miniz_oxide | 0.9.1 | MIT OR Zlib OR Apache-2.0 | https://crates.io/crates/miniz_oxide |
| mio | 1.2.3 | MIT | https://crates.io/crates/mio |
| moxcms | 0.8.1 | BSD-3-Clause OR Apache-2.0 | https://crates.io/crates/moxcms |
| muda | 0.20.0 | Apache-2.0 OR MIT | https://crates.io/crates/muda |
| multiversion_no_op | 1.0.0 | Apache-2.0 OR MIT | https://crates.io/crates/multiversion_no_op |
| munge | 0.4.7 | MIT | https://crates.io/crates/munge |
| munge_macro | 0.4.7 | MIT | https://crates.io/crates/munge_macro |
| murmurhash32 | 0.3.1 | MIT | https://crates.io/crates/murmurhash32 |
| n0-dns-resolver | 0.1.0 | MIT OR Apache-2.0 | https://crates.io/crates/n0-dns-resolver |
| n0-error | 1.0.1 | MIT OR Apache-2.0 | https://crates.io/crates/n0-error |
| n0-error-macros | 1.0.1 | MIT OR Apache-2.0 | https://crates.io/crates/n0-error-macros |
| n0-future | 0.3.2 | MIT OR Apache-2.0 | https://crates.io/crates/n0-future |
| n0-mainline | 0.7.1 | MIT OR Apache-2.0 | https://crates.io/crates/n0-mainline |
| n0-watcher | 1.0.0 | MIT OR Apache-2.0 | https://crates.io/crates/n0-watcher |
| nalgebra | 0.35.0 | Apache-2.0 | https://crates.io/crates/nalgebra |
| nalgebra-macros | 0.3.0 | Apache-2.0 | https://crates.io/crates/nalgebra-macros |
| ndk | 0.9.0 | MIT OR Apache-2.0 | https://crates.io/crates/ndk |
| ndk-context | 0.1.1 | MIT OR Apache-2.0 | https://crates.io/crates/ndk-context |
| ndk-sys | 0.6.0+11769913 | MIT OR Apache-2.0 | https://crates.io/crates/ndk-sys |
| nested_enum_utils | 0.2.3 | MIT OR Apache-2.0 | https://crates.io/crates/nested_enum_utils |
| netdev | 0.45.1 | MIT | https://crates.io/crates/netdev |
| netdev | 0.46.3 | MIT | https://crates.io/crates/netdev |
| netlink-packet-core | 0.8.2 | MIT | https://crates.io/crates/netlink-packet-core |
| netlink-packet-core | 0.9.0 | MIT | https://crates.io/crates/netlink-packet-core |
| netlink-packet-route | 0.31.0 | MIT | https://crates.io/crates/netlink-packet-route |
| netlink-packet-route | 0.33.0 | MIT | https://crates.io/crates/netlink-packet-route |
| netlink-proto | 0.12.2 | MIT | https://crates.io/crates/netlink-proto |
| netlink-sys | 0.8.8 | MIT | https://crates.io/crates/netlink-sys |
| netlink-sys | 0.9.0 | MIT | https://crates.io/crates/netlink-sys |
| netwatch | 0.19.3 | MIT OR Apache-2.0 | https://crates.io/crates/netwatch |
| new_debug_unreachable | 1.0.6 | MIT | https://crates.io/crates/new_debug_unreachable |
| nom | 7.1.3 | MIT | https://crates.io/crates/nom |
| nom | 8.0.0 | MIT | https://crates.io/crates/nom |
| nonempty | 0.7.0 | MIT | https://crates.io/crates/nonempty |
| nonzero_ext | 0.3.0 | Apache-2.0 | https://crates.io/crates/nonzero_ext |
| noq | 1.3.0 | MIT OR Apache-2.0 | https://crates.io/crates/noq |
| noq-proto | 1.3.0 | MIT OR Apache-2.0 | https://crates.io/crates/noq-proto |
| noq-udp | 1.3.0 | MIT OR Apache-2.0 | https://crates.io/crates/noq-udp |
| notify-rust | 4.18.1 | MIT OR Apache-2.0 | https://crates.io/crates/notify-rust |
| nu-ansi-term | 0.50.3 | MIT | https://crates.io/crates/nu-ansi-term |
| num | 0.4.3 | MIT OR Apache-2.0 | https://crates.io/crates/num |
| num-bigint | 0.4.8 | MIT OR Apache-2.0 | https://crates.io/crates/num-bigint |
| num-complex | 0.4.6 | MIT OR Apache-2.0 | https://crates.io/crates/num-complex |
| num-conv | 0.2.2 | MIT OR Apache-2.0 | https://crates.io/crates/num-conv |
| num-derive | 0.5.1 | MIT OR Apache-2.0 | https://crates.io/crates/num-derive |
| num-integer | 0.1.47 | MIT OR Apache-2.0 | https://crates.io/crates/num-integer |
| num-iter | 0.1.46 | MIT OR Apache-2.0 | https://crates.io/crates/num-iter |
| num-rational | 0.4.2 | MIT OR Apache-2.0 | https://crates.io/crates/num-rational |
| num-traits | 0.2.19 | MIT OR Apache-2.0 | https://crates.io/crates/num-traits |
| num_cpus | 1.17.0 | MIT OR Apache-2.0 | https://crates.io/crates/num_cpus |
| num_enum | 0.7.6 | BSD-3-Clause OR MIT OR Apache-2.0 | https://crates.io/crates/num_enum |
| num_enum_derive | 0.7.6 | BSD-3-Clause OR MIT OR Apache-2.0 | https://crates.io/crates/num_enum_derive |
| objc2 | 0.6.4 | MIT | https://crates.io/crates/objc2 |
| objc2-app-kit | 0.3.2 | Zlib OR Apache-2.0 OR MIT | https://crates.io/crates/objc2-app-kit |
| objc2-cloud-kit | 0.3.2 | Zlib OR Apache-2.0 OR MIT | https://crates.io/crates/objc2-cloud-kit |
| objc2-core-data | 0.3.2 | Zlib OR Apache-2.0 OR MIT | https://crates.io/crates/objc2-core-data |
| objc2-core-foundation | 0.3.2 | Zlib OR Apache-2.0 OR MIT | https://crates.io/crates/objc2-core-foundation |
| objc2-core-graphics | 0.3.2 | Zlib OR Apache-2.0 OR MIT | https://crates.io/crates/objc2-core-graphics |
| objc2-core-image | 0.3.2 | Zlib OR Apache-2.0 OR MIT | https://crates.io/crates/objc2-core-image |
| objc2-core-location | 0.3.2 | Zlib OR Apache-2.0 OR MIT | https://crates.io/crates/objc2-core-location |
| objc2-core-text | 0.3.2 | Zlib OR Apache-2.0 OR MIT | https://crates.io/crates/objc2-core-text |
| objc2-core-wlan | 0.3.2 | Zlib OR Apache-2.0 OR MIT | https://crates.io/crates/objc2-core-wlan |
| objc2-encode | 4.1.0 | MIT | https://crates.io/crates/objc2-encode |
| objc2-exception-helper | 0.1.1 | Zlib OR Apache-2.0 OR MIT | https://crates.io/crates/objc2-exception-helper |
| objc2-foundation | 0.3.2 | MIT | https://crates.io/crates/objc2-foundation |
| objc2-io-surface | 0.3.2 | Zlib OR Apache-2.0 OR MIT | https://crates.io/crates/objc2-io-surface |
| objc2-osa-kit | 0.3.2 | Zlib OR Apache-2.0 OR MIT | https://crates.io/crates/objc2-osa-kit |
| objc2-quartz-core | 0.3.2 | Zlib OR Apache-2.0 OR MIT | https://crates.io/crates/objc2-quartz-core |
| objc2-security | 0.3.2 | Zlib OR Apache-2.0 OR MIT | https://crates.io/crates/objc2-security |
| objc2-security-foundation | 0.3.2 | Zlib OR Apache-2.0 OR MIT | https://crates.io/crates/objc2-security-foundation |
| objc2-system-configuration | 0.3.2 | Zlib OR Apache-2.0 OR MIT | https://crates.io/crates/objc2-system-configuration |
| objc2-ui-kit | 0.3.2 | Zlib OR Apache-2.0 OR MIT | https://crates.io/crates/objc2-ui-kit |
| objc2-user-notifications | 0.3.2 | Zlib OR Apache-2.0 OR MIT | https://crates.io/crates/objc2-user-notifications |
| objc2-web-kit | 0.3.2 | Zlib OR Apache-2.0 OR MIT | https://crates.io/crates/objc2-web-kit |
| oid-registry | 0.8.1 | MIT OR Apache-2.0 | https://crates.io/crates/oid-registry |
| once_cell | 1.21.4 | MIT OR Apache-2.0 | https://crates.io/crates/once_cell |
| once_cell_polyfill | 1.70.2 | MIT OR Apache-2.0 | https://crates.io/crates/once_cell_polyfill |
| oneshot | 0.1.13 | MIT OR Apache-2.0 | https://crates.io/crates/oneshot |
| oorandom | 11.1.5 | MIT | https://crates.io/crates/oorandom |
| opaque-debug | 0.3.1 | MIT OR Apache-2.0 | https://crates.io/crates/opaque-debug |
| open | 5.4.4 | MIT | https://crates.io/crates/open |
| openssl-probe | 0.2.1 | MIT OR Apache-2.0 | https://crates.io/crates/openssl-probe |
| option-ext | 0.2.0 | MPL-2.0 | https://crates.io/crates/option-ext |
| ordered-float | 5.5.0 | MIT | https://crates.io/crates/ordered-float |
| ordered-multimap | 0.7.3 | MIT | https://crates.io/crates/ordered-multimap |
| ordered-stream | 0.2.0 | MIT OR Apache-2.0 | https://crates.io/crates/ordered-stream |
| osakit | 0.3.1 | MIT OR Apache-2.0 | https://crates.io/crates/osakit |
| ownedbytes | 0.9.0 | MIT | https://crates.io/crates/ownedbytes |
| p256 | 0.13.2 | Apache-2.0 OR MIT | https://crates.io/crates/p256 |
| p384 | 0.13.1 | Apache-2.0 OR MIT | https://crates.io/crates/p384 |
| pango | 0.18.3 | MIT | https://crates.io/crates/pango |
| pango-sys | 0.18.0 | MIT | https://crates.io/crates/pango-sys |
| papaya | 0.2.5 | MIT | https://crates.io/crates/papaya |
| parking | 2.2.1 | Apache-2.0 OR MIT | https://crates.io/crates/parking |
| parking_lot | 0.12.5 | MIT OR Apache-2.0 | https://crates.io/crates/parking_lot |
| parking_lot_core | 0.9.12 | MIT OR Apache-2.0 | https://crates.io/crates/parking_lot_core |
| parry3d | 0.31.1 | Apache-2.0 | https://crates.io/crates/parry3d |
| password-hash | 0.6.1 | MIT OR Apache-2.0 | https://crates.io/crates/password-hash |
| paste | 1.0.15 | MIT OR Apache-2.0 | https://crates.io/crates/paste |
| pem | 3.0.6 | MIT | https://crates.io/crates/pem |
| pem | 4.0.0 | MIT | https://crates.io/crates/pem |
| pem-rfc7468 | 0.7.0 | Apache-2.0 OR MIT | https://crates.io/crates/pem-rfc7468 |
| pem-rfc7468 | 1.0.0 | Apache-2.0 OR MIT | https://crates.io/crates/pem-rfc7468 |
| percent-encoding | 2.3.2 | MIT OR Apache-2.0 | https://crates.io/crates/percent-encoding |
| pharos | 0.5.3 | Unlicense | https://crates.io/crates/pharos |
| phc | 0.6.1 | Apache-2.0 OR MIT | https://crates.io/crates/phc |
| phf | 0.13.1 | MIT | https://crates.io/crates/phf |
| phf | 0.14.0 | MIT | https://crates.io/crates/phf |
| phf_codegen | 0.13.1 | MIT | https://crates.io/crates/phf_codegen |
| phf_codegen | 0.14.0 | MIT | https://crates.io/crates/phf_codegen |
| phf_generator | 0.13.1 | MIT | https://crates.io/crates/phf_generator |
| phf_generator | 0.14.0 | MIT | https://crates.io/crates/phf_generator |
| phf_macros | 0.13.1 | MIT | https://crates.io/crates/phf_macros |
| phf_shared | 0.13.1 | MIT | https://crates.io/crates/phf_shared |
| phf_shared | 0.14.0 | MIT | https://crates.io/crates/phf_shared |
| pin-project | 1.1.13 | Apache-2.0 OR MIT | https://crates.io/crates/pin-project |
| pin-project-internal | 1.1.13 | Apache-2.0 OR MIT | https://crates.io/crates/pin-project-internal |
| pin-project-lite | 0.2.17 | Apache-2.0 OR MIT | https://crates.io/crates/pin-project-lite |
| piper | 0.2.5 | MIT OR Apache-2.0 | https://crates.io/crates/piper |
| pkcs8 | 0.10.2 | Apache-2.0 OR MIT | https://crates.io/crates/pkcs8 |
| pkcs8 | 0.11.0 | Apache-2.0 OR MIT | https://crates.io/crates/pkcs8 |
| pkg-config | 0.3.34 | MIT OR Apache-2.0 | https://crates.io/crates/pkg-config |
| plist | 1.10.1 | MIT | https://crates.io/crates/plist |
| png | 0.17.16 | MIT OR Apache-2.0 | https://crates.io/crates/png |
| png | 0.18.1 | MIT OR Apache-2.0 | https://crates.io/crates/png |
| polling | 3.11.0 | Apache-2.0 OR MIT | https://crates.io/crates/polling |
| poly1305 | 0.8.0 | Apache-2.0 OR MIT | https://crates.io/crates/poly1305 |
| poly1305 | 0.9.1 | Apache-2.0 OR MIT | https://crates.io/crates/poly1305 |
| polyval | 0.6.2 | Apache-2.0 OR MIT | https://crates.io/crates/polyval |
| portable-atomic | 1.15.0 | Apache-2.0 OR MIT | https://crates.io/crates/portable-atomic |
| portable-atomic-util | 0.2.8 | Apache-2.0 OR MIT | https://crates.io/crates/portable-atomic-util |
| portmapper | 0.19.3 | MIT OR Apache-2.0 | https://crates.io/crates/portmapper |
| positioned-io | 0.3.5 | MIT | https://crates.io/crates/positioned-io |
| postcard | 1.1.3 | MIT OR Apache-2.0 | https://crates.io/crates/postcard |
| postcard-derive | 0.2.2 | MIT OR Apache-2.0 | https://crates.io/crates/postcard-derive |
| potential_utf | 0.1.6 | Unicode-3.0 | https://crates.io/crates/potential_utf |
| powerfmt | 0.2.0 | MIT OR Apache-2.0 | https://crates.io/crates/powerfmt |
| ppv-lite86 | 0.2.21 | MIT OR Apache-2.0 | https://crates.io/crates/ppv-lite86 |
| precomputed-hash | 0.1.1 | MIT | https://crates.io/crates/precomputed-hash |
| prettyplease | 0.3.0 | MIT OR Apache-2.0 | https://crates.io/crates/prettyplease |
| primeorder | 0.13.6 | Apache-2.0 OR MIT | https://crates.io/crates/primeorder |
| proc-macro-crate | 1.3.1 | MIT OR Apache-2.0 | https://crates.io/crates/proc-macro-crate |
| proc-macro-crate | 2.0.2 | MIT OR Apache-2.0 | https://crates.io/crates/proc-macro-crate |
| proc-macro-crate | 3.5.0 | MIT OR Apache-2.0 | https://crates.io/crates/proc-macro-crate |
| proc-macro-error | 0.4.12 | MIT OR Apache-2.0 | https://crates.io/crates/proc-macro-error |
| proc-macro-error | 1.0.4 | MIT OR Apache-2.0 | https://crates.io/crates/proc-macro-error |
| proc-macro-error-attr | 0.4.12 | MIT OR Apache-2.0 | https://crates.io/crates/proc-macro-error-attr |
| proc-macro-error-attr | 1.0.4 | MIT OR Apache-2.0 | https://crates.io/crates/proc-macro-error-attr |
| proc-macro-hack | 0.5.20+deprecated | MIT OR Apache-2.0 | https://crates.io/crates/proc-macro-hack |
| proc-macro2 | 1.0.107 | MIT OR Apache-2.0 | https://crates.io/crates/proc-macro2 |
| profiling | 1.0.18 | MIT OR Apache-2.0 | https://crates.io/crates/profiling |
| profiling-procmacros | 1.0.18 | MIT OR Apache-2.0 | https://crates.io/crates/profiling-procmacros |
| ptr_meta | 0.3.2 | MIT | https://crates.io/crates/ptr_meta |
| ptr_meta_derive | 0.3.2 | MIT | https://crates.io/crates/ptr_meta_derive |
| pxfm | 0.1.30 | BSD-3-Clause OR Apache-2.0 | https://crates.io/crates/pxfm |
| quanta | 0.12.6 | MIT | https://crates.io/crates/quanta |
| quick-error | 2.0.1 | MIT/Apache-2.0 | https://crates.io/crates/quick-error |
| quick-xml | 0.42.0 | MIT | https://crates.io/crates/quick-xml |
| quinn | 0.11.12 | MIT OR Apache-2.0 | https://crates.io/crates/quinn |
| quinn-proto | 0.11.19 | MIT OR Apache-2.0 | https://crates.io/crates/quinn-proto |
| quinn-udp | 0.5.16 | MIT OR Apache-2.0 | https://crates.io/crates/quinn-udp |
| quote | 1.0.47 | MIT OR Apache-2.0 | https://crates.io/crates/quote |
| r-efi | 5.3.0 | MIT OR Apache-2.0 OR LGPL-2.1-or-later | https://crates.io/crates/r-efi |
| r-efi | 6.0.0 | MIT OR Apache-2.0 OR LGPL-2.1-or-later | https://crates.io/crates/r-efi |
| rancor | 0.1.3 | MIT | https://crates.io/crates/rancor |
| rand | 0.10.3 | MIT OR Apache-2.0 | https://crates.io/crates/rand |
| rand | 0.9.5 | MIT OR Apache-2.0 | https://crates.io/crates/rand |
| rand_chacha | 0.9.0 | MIT OR Apache-2.0 | https://crates.io/crates/rand_chacha |
| rand_core | 0.10.1 | MIT OR Apache-2.0 | https://crates.io/crates/rand_core |
| rand_core | 0.6.4 | MIT OR Apache-2.0 | https://crates.io/crates/rand_core |
| rand_core | 0.9.5 | MIT OR Apache-2.0 | https://crates.io/crates/rand_core |
| rand_pcg | 0.10.2 | MIT OR Apache-2.0 | https://crates.io/crates/rand_pcg |
| range-collections | 0.4.6 | MIT OR Apache-2.0 | https://crates.io/crates/range-collections |
| rapier3d | 0.36.0 | Apache-2.0 | https://crates.io/crates/rapier3d |
| raw-cpuid | 11.6.0 | MIT | https://crates.io/crates/raw-cpuid |
| raw-window-handle | 0.6.2 | MIT OR Apache-2.0 OR Zlib | https://crates.io/crates/raw-window-handle |
| rawpointer | 0.2.1 | MIT/Apache-2.0 | https://crates.io/crates/rawpointer |
| rayon | 1.12.0 | MIT OR Apache-2.0 | https://crates.io/crates/rayon |
| rayon-core | 1.13.0 | MIT OR Apache-2.0 | https://crates.io/crates/rayon-core |
| rcgen | 0.14.10 | MIT OR Apache-2.0 | https://crates.io/crates/rcgen |
| redb | 3.1.3 | MIT OR Apache-2.0 | https://crates.io/crates/redb |
| redb | 4.3.0 | MIT OR Apache-2.0 | https://crates.io/crates/redb |
| redis | 1.7.1 | BSD-3-Clause | https://crates.io/crates/redis |
| redox_syscall | 0.5.18 | MIT | https://crates.io/crates/redox_syscall |
| redox_users | 0.5.3 | MIT | https://crates.io/crates/redox_users |
| ref-cast | 1.0.27 | MIT OR Apache-2.0 | https://crates.io/crates/ref-cast |
| ref-cast-impl | 1.0.27 | MIT OR Apache-2.0 | https://crates.io/crates/ref-cast-impl |
| reflink-copy | 0.1.30 | MIT/Apache-2.0 | https://crates.io/crates/reflink-copy |
| regex | 1.13.1 | MIT OR Apache-2.0 | https://crates.io/crates/regex |
| regex-automata | 0.4.18 | MIT OR Apache-2.0 | https://crates.io/crates/regex-automata |
| regex-syntax | 0.8.11 | MIT OR Apache-2.0 | https://crates.io/crates/regex-syntax |
| reloadable-core | 0.1.0 | MIT | https://crates.io/crates/reloadable-core |
| reloadable-state | 0.1.0 | MIT | https://crates.io/crates/reloadable-state |
| rend | 0.5.4 | MIT | https://crates.io/crates/rend |
| reqwest | 0.13.5 | MIT OR Apache-2.0 | https://crates.io/crates/reqwest |
| rfc6979 | 0.4.0 | Apache-2.0 OR MIT | https://crates.io/crates/rfc6979 |
| rfd | 0.16.0 | MIT | https://crates.io/crates/rfd |
| ring | 0.17.14 | Apache-2.0 AND ISC | https://crates.io/crates/ring |
| rkyv | 0.8.18 | MIT | https://crates.io/crates/rkyv |
| rkyv_derive | 0.8.18 | MIT | https://crates.io/crates/rkyv_derive |
| robust | 1.2.0 | MIT OR Apache-2.0 | https://crates.io/crates/robust |
| rstar | 0.13.0 | MIT OR Apache-2.0 | https://crates.io/crates/rstar |
| rust-ini | 0.21.3 | MIT | https://crates.io/crates/rust-ini |
| rust-stemmers | 1.2.0 | MIT/BSD-3-Clause | https://crates.io/crates/rust-stemmers |
| rustc-hash | 2.1.3 | Apache-2.0 OR MIT | https://crates.io/crates/rustc-hash |
| rustc_version | 0.4.1 | MIT OR Apache-2.0 | https://crates.io/crates/rustc_version |
| rusticata-macros | 4.1.0 | MIT/Apache-2.0 | https://crates.io/crates/rusticata-macros |
| rustix | 1.1.5 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | https://crates.io/crates/rustix |
| rustls | 0.23.45 | Apache-2.0 OR ISC OR MIT | https://crates.io/crates/rustls |
| rustls-cert-file-reader | 0.4.2 | MIT | https://crates.io/crates/rustls-cert-file-reader |
| rustls-cert-read | 0.3.0 | MIT | https://crates.io/crates/rustls-cert-read |
| rustls-cert-reloadable-resolver | 0.7.1 | MIT | https://crates.io/crates/rustls-cert-reloadable-resolver |
| rustls-native-certs | 0.8.4 | Apache-2.0 OR ISC OR MIT | https://crates.io/crates/rustls-native-certs |
| rustls-pki-types | 1.15.1 | MIT OR Apache-2.0 | https://crates.io/crates/rustls-pki-types |
| rustls-platform-verifier | 0.7.1 | MIT OR Apache-2.0 | https://crates.io/crates/rustls-platform-verifier |
| rustls-platform-verifier-android | 0.2.0 | MIT OR Apache-2.0 | https://crates.io/crates/rustls-platform-verifier-android |
| rustls-webpki | 0.103.15 | ISC | https://crates.io/crates/rustls-webpki |
| rustversion | 1.0.23 | MIT OR Apache-2.0 | https://crates.io/crates/rustversion |
| ryu | 1.0.23 | Apache-2.0 OR BSL-1.0 | https://crates.io/crates/ryu |
| safe_arch | 1.2.0 | Zlib OR Apache-2.0 OR MIT | https://crates.io/crates/safe_arch |
| same-file | 1.0.6 | Unlicense/MIT | https://crates.io/crates/same-file |
| schannel | 0.1.29 | MIT | https://crates.io/crates/schannel |
| schemars | 0.8.22 | MIT | https://crates.io/crates/schemars |
| schemars | 0.9.0 | MIT | https://crates.io/crates/schemars |
| schemars | 1.2.2 | MIT | https://crates.io/crates/schemars |
| schemars_derive | 0.8.22 | MIT | https://crates.io/crates/schemars_derive |
| scoped-tls | 1.0.1 | MIT/Apache-2.0 | https://crates.io/crates/scoped-tls |
| scopeguard | 1.2.0 | MIT OR Apache-2.0 | https://crates.io/crates/scopeguard |
| sctp-proto | 0.10.5 | MIT OR Apache-2.0 | https://crates.io/crates/sctp-proto |
| sec1 | 0.7.3 | Apache-2.0 OR MIT | https://crates.io/crates/sec1 |
| secp256k1 | 0.33.1 | CC0-1.0 | https://crates.io/crates/secp256k1 |
| secp256k1-sys | 0.14.1 | CC0-1.0 | https://crates.io/crates/secp256k1-sys |
| secret-service | 5.2.0 | MIT OR Apache-2.0 | https://crates.io/crates/secret-service |
| security-framework | 3.7.0 | MIT OR Apache-2.0 | https://crates.io/crates/security-framework |
| security-framework-sys | 2.17.0 | MIT OR Apache-2.0 | https://crates.io/crates/security-framework-sys |
| seize | 0.5.1 | MIT | https://crates.io/crates/seize |
| selectors | 0.38.0 | MPL-2.0 | https://crates.io/crates/selectors |
| self_cell | 1.3.0 | Apache-2.0 OR GPL-2.0-only | https://crates.io/crates/self_cell |
| semver | 1.0.28 | MIT OR Apache-2.0 | https://crates.io/crates/semver |
| send_wrapper | 0.6.0 | MIT/Apache-2.0 | https://crates.io/crates/send_wrapper |
| serde | 1.0.229 | MIT OR Apache-2.0 | https://crates.io/crates/serde |
| serde-error | 0.1.3 | MIT | https://crates.io/crates/serde-error |
| serde-untagged | 0.1.9 | MIT OR Apache-2.0 | https://crates.io/crates/serde-untagged |
| serde_bencode | 0.2.4 | MIT | https://crates.io/crates/serde_bencode |
| serde_bytes | 0.11.19 | MIT OR Apache-2.0 | https://crates.io/crates/serde_bytes |
| serde_core | 1.0.229 | MIT OR Apache-2.0 | https://crates.io/crates/serde_core |
| serde_derive | 1.0.229 | MIT OR Apache-2.0 | https://crates.io/crates/serde_derive |
| serde_derive_internals | 0.29.1 | MIT OR Apache-2.0 | https://crates.io/crates/serde_derive_internals |
| serde_json | 1.0.151 | MIT OR Apache-2.0 | https://crates.io/crates/serde_json |
| serde_path_to_error | 0.1.20 | MIT OR Apache-2.0 | https://crates.io/crates/serde_path_to_error |
| serde_repr | 0.1.21 | MIT OR Apache-2.0 | https://crates.io/crates/serde_repr |
| serde_spanned | 0.6.9 | MIT OR Apache-2.0 | https://crates.io/crates/serde_spanned |
| serde_spanned | 1.1.1 | MIT OR Apache-2.0 | https://crates.io/crates/serde_spanned |
| serde_urlencoded | 0.7.1 | MIT/Apache-2.0 | https://crates.io/crates/serde_urlencoded |
| serde_with | 3.24.0 | MIT OR Apache-2.0 | https://crates.io/crates/serde_with |
| serde_with_macros | 3.24.0 | MIT OR Apache-2.0 | https://crates.io/crates/serde_with_macros |
| serde_yaml | 0.9.34+deprecated | MIT OR Apache-2.0 | https://crates.io/crates/serde_yaml |
| serde_yaml_ng | 0.10.0 | MIT | https://crates.io/crates/serde_yaml_ng |
| serdect | 0.4.3 | Apache-2.0 OR MIT | https://crates.io/crates/serdect |
| serialize-to-javascript | 0.1.2 | MIT OR Apache-2.0 | https://crates.io/crates/serialize-to-javascript |
| serialize-to-javascript-impl | 0.1.2 | MIT OR Apache-2.0 | https://crates.io/crates/serialize-to-javascript-impl |
| servo_arc | 0.4.3 | MIT OR Apache-2.0 | https://crates.io/crates/servo_arc |
| sha1 | 0.10.7 | MIT OR Apache-2.0 | https://crates.io/crates/sha1 |
| sha1 | 0.11.0 | MIT OR Apache-2.0 | https://crates.io/crates/sha1 |
| sha1_smol | 1.0.1 | BSD-3-Clause | https://crates.io/crates/sha1_smol |
| sha2 | 0.10.9 | MIT OR Apache-2.0 | https://crates.io/crates/sha2 |
| sha2 | 0.11.0 | MIT OR Apache-2.0 | https://crates.io/crates/sha2 |
| sharded-slab | 0.1.7 | MIT | https://crates.io/crates/sharded-slab |
| shlex | 2.0.1 | MIT OR Apache-2.0 | https://crates.io/crates/shlex |
| signal-hook-registry | 1.4.8 | MIT OR Apache-2.0 | https://crates.io/crates/signal-hook-registry |
| signature | 2.2.0 | Apache-2.0 OR MIT | https://crates.io/crates/signature |
| signature | 3.0.0 | Apache-2.0 OR MIT | https://crates.io/crates/signature |
| simba | 0.10.2 | Apache-2.0 | https://crates.io/crates/simba |
| simd-adler32 | 0.3.10 | MIT | https://crates.io/crates/simd-adler32 |
| simd_cesu8 | 1.2.0 | Apache-2.0 OR MIT | https://crates.io/crates/simd_cesu8 |
| simdutf8 | 0.1.5 | MIT OR Apache-2.0 | https://crates.io/crates/simdutf8 |
| simple-dns | 0.12.0 | MIT | https://crates.io/crates/simple-dns |
| simple_asn1 | 0.6.4 | ISC | https://crates.io/crates/simple_asn1 |
| siphasher | 1.0.4 | MIT OR Apache-2.0 | https://crates.io/crates/siphasher |
| sketches-ddsketch | 0.4.1 | Apache-2.0 | https://crates.io/crates/sketches-ddsketch |
| slab | 0.4.12 | MIT | https://crates.io/crates/slab |
| smallvec | 1.16.2 | MIT OR Apache-2.0 | https://crates.io/crates/smallvec |
| socket2 | 0.6.5 | MIT OR Apache-2.0 | https://crates.io/crates/socket2 |
| softbuffer | 0.4.8 | MIT OR Apache-2.0 | https://crates.io/crates/softbuffer |
| sorted-index-buffer | 0.2.1 | MIT OR Apache-2.0 | https://crates.io/crates/sorted-index-buffer |
| soup3 | 0.5.0 | MIT | https://crates.io/crates/soup3 |
| soup3-sys | 0.5.0 | MIT | https://crates.io/crates/soup3-sys |
| spade | 2.15.1 | MIT OR Apache-2.0 | https://crates.io/crates/spade |
| spez | 0.1.2 | BSD-2-Clause | https://crates.io/crates/spez |
| spin | 0.10.1 | MIT | https://crates.io/crates/spin |
| spin | 0.9.9 | MIT | https://crates.io/crates/spin |
| spinning_top | 0.3.0 | MIT/Apache-2.0 | https://crates.io/crates/spinning_top |
| spki | 0.7.3 | Apache-2.0 OR MIT | https://crates.io/crates/spki |
| spki | 0.8.0 | Apache-2.0 OR MIT | https://crates.io/crates/spki |
| sqlx | 0.9.0 | MIT OR Apache-2.0 | https://crates.io/crates/sqlx |
| sqlx-core | 0.9.0 | MIT OR Apache-2.0 | https://crates.io/crates/sqlx-core |
| sqlx-macros | 0.9.0 | MIT OR Apache-2.0 | https://crates.io/crates/sqlx-macros |
| sqlx-macros-core | 0.9.0 | MIT OR Apache-2.0 | https://crates.io/crates/sqlx-macros-core |
| sqlx-mysql | 0.9.0 | MIT OR Apache-2.0 | https://crates.io/crates/sqlx-mysql |
| sqlx-postgres | 0.9.0 | MIT OR Apache-2.0 | https://crates.io/crates/sqlx-postgres |
| sqlx-sqlite | 0.9.0 | MIT OR Apache-2.0 | https://crates.io/crates/sqlx-sqlite |
| stable_deref_trait | 1.2.1 | MIT OR Apache-2.0 | https://crates.io/crates/stable_deref_trait |
| static_assertions | 1.1.0 | MIT OR Apache-2.0 | https://crates.io/crates/static_assertions |
| str0m | 0.24.0 | MIT OR Apache-2.0 | https://crates.io/crates/str0m |
| str0m-proto | 0.7.0 | MIT OR Apache-2.0 | https://crates.io/crates/str0m-proto |
| str0m-rust-crypto | 0.6.0 | MIT OR Apache-2.0 | https://crates.io/crates/str0m-rust-crypto |
| string_cache | 0.11.0 | MIT OR Apache-2.0 | https://crates.io/crates/string_cache |
| string_cache | 0.9.0 | MIT OR Apache-2.0 | https://crates.io/crates/string_cache |
| string_cache_codegen | 0.11.2 | MIT OR Apache-2.0 | https://crates.io/crates/string_cache_codegen |
| string_cache_codegen | 0.6.1 | MIT OR Apache-2.0 | https://crates.io/crates/string_cache_codegen |
| stringprep | 0.1.5 | MIT/Apache-2.0 | https://crates.io/crates/stringprep |
| strsim | 0.11.1 | MIT | https://crates.io/crates/strsim |
| strum | 0.28.0 | MIT | https://crates.io/crates/strum |
| strum_macros | 0.28.0 | MIT | https://crates.io/crates/strum_macros |
| subtle | 2.6.1 | BSD-3-Clause | https://crates.io/crates/subtle |
| swift-rs | 1.0.8 | MIT OR Apache-2.0 | https://crates.io/crates/swift-rs |
| syn | 1.0.109 | MIT OR Apache-2.0 | https://crates.io/crates/syn |
| syn | 2.0.119 | MIT OR Apache-2.0 | https://crates.io/crates/syn |
| syn | 3.0.6 | MIT OR Apache-2.0 | https://crates.io/crates/syn |
| syn-mid | 0.5.4 | Apache-2.0 OR MIT | https://crates.io/crates/syn-mid |
| sync_wrapper | 1.0.2 | Apache-2.0 | https://crates.io/crates/sync_wrapper |
| synstructure | 0.13.2 | MIT | https://crates.io/crates/synstructure |
| synstructure | 0.14.0 | MIT | https://crates.io/crates/synstructure |
| system-configuration | 0.7.0 | MIT OR Apache-2.0 | https://crates.io/crates/system-configuration |
| system-configuration | 0.8.0 | MIT OR Apache-2.0 | https://crates.io/crates/system-configuration |
| system-configuration-sys | 0.6.0 | MIT OR Apache-2.0 | https://crates.io/crates/system-configuration-sys |
| system-deps | 6.2.2 | MIT OR Apache-2.0 | https://crates.io/crates/system-deps |
| tantivy | 0.26.2 | MIT | https://crates.io/crates/tantivy |
| tantivy-bitpacker | 0.10.0 | MIT | https://crates.io/crates/tantivy-bitpacker |
| tantivy-columnar | 0.7.0 | MIT | https://crates.io/crates/tantivy-columnar |
| tantivy-common | 0.11.0 | MIT | https://crates.io/crates/tantivy-common |
| tantivy-fst | 0.5.0 | Unlicense/MIT | https://crates.io/crates/tantivy-fst |
| tantivy-query-grammar | 0.26.0 | MIT | https://crates.io/crates/tantivy-query-grammar |
| tantivy-sstable | 0.7.0 | MIT | https://crates.io/crates/tantivy-sstable |
| tantivy-stacker | 0.7.0 | MIT | https://crates.io/crates/tantivy-stacker |
| tantivy-tokenizer-api | 0.7.0 | MIT | https://crates.io/crates/tantivy-tokenizer-api |
| tao | 0.37.1 | Apache-2.0 | https://crates.io/crates/tao |
| tao-macros | 0.1.4 | MIT OR Apache-2.0 | https://crates.io/crates/tao-macros |
| tar | 0.4.46 | MIT OR Apache-2.0 | https://crates.io/crates/tar |
| target-lexicon | 0.12.16 | Apache-2.0 WITH LLVM-exception | https://crates.io/crates/target-lexicon |
| tauri | 2.12.0 | Apache-2.0 OR MIT | https://crates.io/crates/tauri |
| tauri-build | 2.7.0 | Apache-2.0 OR MIT | https://crates.io/crates/tauri-build |
| tauri-codegen | 2.7.0 | Apache-2.0 OR MIT | https://crates.io/crates/tauri-codegen |
| tauri-macros | 2.7.0 | Apache-2.0 OR MIT | https://crates.io/crates/tauri-macros |
| tauri-plugin | 2.7.0 | Apache-2.0 OR MIT | https://crates.io/crates/tauri-plugin |
| tauri-plugin-deep-link | 2.6.0 | Apache-2.0 OR MIT | https://crates.io/crates/tauri-plugin-deep-link |
| tauri-plugin-dialog | 2.8.0 | Apache-2.0 OR MIT | https://crates.io/crates/tauri-plugin-dialog |
| tauri-plugin-fs | 2.6.0 | Apache-2.0 OR MIT | https://crates.io/crates/tauri-plugin-fs |
| tauri-plugin-single-instance | 2.5.1 | Apache-2.0 OR MIT | https://crates.io/crates/tauri-plugin-single-instance |
| tauri-plugin-updater | 2.13.1 | Apache-2.0 OR MIT | https://crates.io/crates/tauri-plugin-updater |
| tauri-runtime | 2.12.0 | Apache-2.0 OR MIT | https://crates.io/crates/tauri-runtime |
| tauri-runtime-wry | 2.12.0 | Apache-2.0 OR MIT | https://crates.io/crates/tauri-runtime-wry |
| tauri-utils | 2.10.0 | Apache-2.0 OR MIT | https://crates.io/crates/tauri-utils |
| tauri-winres | 0.3.6 | MIT | https://crates.io/crates/tauri-winres |
| tauri-winrt-notification | 0.8.1 | MIT OR Apache-2.0 | https://crates.io/crates/tauri-winrt-notification |
| tempfile | 3.27.0 | MIT OR Apache-2.0 | https://crates.io/crates/tempfile |
| tendril | 0.5.1 | MIT OR Apache-2.0 | https://crates.io/crates/tendril |
| thiserror | 1.0.69 | MIT OR Apache-2.0 | https://crates.io/crates/thiserror |
| thiserror | 2.0.21 | MIT OR Apache-2.0 | https://crates.io/crates/thiserror |
| thiserror-impl | 1.0.69 | MIT OR Apache-2.0 | https://crates.io/crates/thiserror-impl |
| thiserror-impl | 2.0.21 | MIT OR Apache-2.0 | https://crates.io/crates/thiserror-impl |
| thread_local | 1.1.10 | MIT OR Apache-2.0 | https://crates.io/crates/thread_local |
| time | 0.3.55 | MIT OR Apache-2.0 | https://crates.io/crates/time |
| time-core | 0.1.9 | MIT OR Apache-2.0 | https://crates.io/crates/time-core |
| time-macros | 0.2.32 | MIT OR Apache-2.0 | https://crates.io/crates/time-macros |
| tiny-keccak | 2.0.2 | CC0-1.0 | https://crates.io/crates/tiny-keccak |
| tinystr | 0.8.4 | Unicode-3.0 | https://crates.io/crates/tinystr |
| tinyvec | 1.13.3 | Zlib OR Apache-2.0 OR MIT | https://crates.io/crates/tinyvec |
| tokio | 1.53.1 | MIT | https://crates.io/crates/tokio |
| tokio-macros | 2.7.2 | MIT | https://crates.io/crates/tokio-macros |
| tokio-rustls | 0.26.6 | MIT OR Apache-2.0 | https://crates.io/crates/tokio-rustls |
| tokio-rustls-acme | 0.9.1 | Apache-2.0 OR MIT | https://crates.io/crates/tokio-rustls-acme |
| tokio-stream | 0.1.19 | MIT | https://crates.io/crates/tokio-stream |
| tokio-tungstenite | 0.29.0 | MIT | https://crates.io/crates/tokio-tungstenite |
| tokio-util | 0.7.19 | MIT | https://crates.io/crates/tokio-util |
| tokio-websockets | 0.13.3 | MIT | https://crates.io/crates/tokio-websockets |
| toml | 0.8.2 | MIT OR Apache-2.0 | https://crates.io/crates/toml |
| toml | 1.1.6+spec-1.1.0 | MIT OR Apache-2.0 | https://crates.io/crates/toml |
| toml_datetime | 0.6.3 | MIT OR Apache-2.0 | https://crates.io/crates/toml_datetime |
| toml_datetime | 1.1.1+spec-1.1.0 | MIT OR Apache-2.0 | https://crates.io/crates/toml_datetime |
| toml_edit | 0.19.15 | MIT OR Apache-2.0 | https://crates.io/crates/toml_edit |
| toml_edit | 0.20.2 | MIT OR Apache-2.0 | https://crates.io/crates/toml_edit |
| toml_edit | 0.25.15+spec-1.1.0 | MIT OR Apache-2.0 | https://crates.io/crates/toml_edit |
| toml_parser | 1.1.3+spec-1.1.0 | MIT OR Apache-2.0 | https://crates.io/crates/toml_parser |
| toml_writer | 1.1.2+spec-1.1.0 | MIT OR Apache-2.0 | https://crates.io/crates/toml_writer |
| tonic | 0.14.6 | MIT | https://crates.io/crates/tonic |
| tower | 0.5.3 | MIT | https://crates.io/crates/tower |
| tower-http | 0.6.11 | MIT | https://crates.io/crates/tower-http |
| tower-http | 0.7.1 | MIT | https://crates.io/crates/tower-http |
| tower-layer | 0.3.3 | MIT | https://crates.io/crates/tower-layer |
| tower-service | 0.3.3 | MIT | https://crates.io/crates/tower-service |
| tower_governor | 0.8.0 | MIT OR Apache-2.0 | https://crates.io/crates/tower_governor |
| tracing | 0.1.44 | MIT | https://crates.io/crates/tracing |
| tracing-attributes | 0.1.31 | MIT | https://crates.io/crates/tracing-attributes |
| tracing-core | 0.1.36 | MIT | https://crates.io/crates/tracing-core |
| tracing-log | 0.2.0 | MIT | https://crates.io/crates/tracing-log |
| tracing-subscriber | 0.3.23 | MIT | https://crates.io/crates/tracing-subscriber |
| tray-icon | 0.25.1 | MIT OR Apache-2.0 | https://crates.io/crates/tray-icon |
| try-lock | 0.2.5 | MIT | https://crates.io/crates/try-lock |
| tungstenite | 0.29.0 | MIT OR Apache-2.0 | https://crates.io/crates/tungstenite |
| typeid | 1.0.3 | MIT OR Apache-2.0 | https://crates.io/crates/typeid |
| typenum | 1.20.1 | MIT OR Apache-2.0 | https://crates.io/crates/typenum |
| typetag | 0.2.23 | MIT OR Apache-2.0 | https://crates.io/crates/typetag |
| typetag-impl | 0.2.23 | MIT OR Apache-2.0 | https://crates.io/crates/typetag-impl |
| udp-addr-index | 0.1.3 | MIT OR Apache-2.0 | https://crates.io/crates/udp-addr-index |
| udp-addr-index-proto | 0.1.3 | MIT OR Apache-2.0 | https://crates.io/crates/udp-addr-index-proto |
| uds_windows | 1.2.1 | MIT | https://crates.io/crates/uds_windows |
| unicase | 2.9.0 | MIT OR Apache-2.0 | https://crates.io/crates/unicase |
| unicode-bidi | 0.3.18 | MIT OR Apache-2.0 | https://crates.io/crates/unicode-bidi |
| unicode-blocks | 0.1.10 | MIT | https://crates.io/crates/unicode-blocks |
| unicode-ident | 1.0.26 | (MIT OR Apache-2.0) AND Unicode-3.0 | https://crates.io/crates/unicode-ident |
| unicode-normalization | 0.1.25 | MIT OR Apache-2.0 | https://crates.io/crates/unicode-normalization |
| unicode-properties | 0.1.4 | MIT/Apache-2.0 | https://crates.io/crates/unicode-properties |
| unicode-segmentation | 1.13.3 | MIT OR Apache-2.0 | https://crates.io/crates/unicode-segmentation |
| unicode-xid | 0.2.6 | MIT OR Apache-2.0 | https://crates.io/crates/unicode-xid |
| universal-hash | 0.5.1 | MIT OR Apache-2.0 | https://crates.io/crates/universal-hash |
| universal-hash | 0.6.1 | MIT OR Apache-2.0 | https://crates.io/crates/universal-hash |
| unsafe-libyaml | 0.2.11 | MIT | https://crates.io/crates/unsafe-libyaml |
| untrusted | 0.7.1 | ISC | https://crates.io/crates/untrusted |
| untrusted | 0.9.0 | ISC | https://crates.io/crates/untrusted |
| ureq | 3.4.2 | MIT OR Apache-2.0 | https://crates.io/crates/ureq |
| ureq-proto | 0.6.4 | MIT OR Apache-2.0 | https://crates.io/crates/ureq-proto |
| url | 2.5.8 | MIT OR Apache-2.0 | https://crates.io/crates/url |
| urlpattern | 0.6.0 | MIT | https://crates.io/crates/urlpattern |
| utf8-ranges | 1.0.5 | Unlicense/MIT | https://crates.io/crates/utf8-ranges |
| utf8-zero | 0.8.1 | MIT OR Apache-2.0 | https://crates.io/crates/utf8-zero |
| utf8_iter | 1.0.4 | Apache-2.0 OR MIT | https://crates.io/crates/utf8_iter |
| utf8parse | 0.2.2 | Apache-2.0 OR MIT | https://crates.io/crates/utf8parse |
| uuid | 1.26.1 | Apache-2.0 OR MIT | https://crates.io/crates/uuid |
| valuable | 0.1.1 | MIT | https://crates.io/crates/valuable |
| vcpkg | 0.2.15 | MIT/Apache-2.0 | https://crates.io/crates/vcpkg |
| version-compare | 0.2.1 | MIT | https://crates.io/crates/version-compare |
| version_check | 0.9.5 | MIT/Apache-2.0 | https://crates.io/crates/version_check |
| vswhom | 0.1.0 | MIT | https://crates.io/crates/vswhom |
| vswhom-sys | 0.1.3 | MIT | https://crates.io/crates/vswhom-sys |
| walkdir | 2.5.0 | Unlicense/MIT | https://crates.io/crates/walkdir |
| want | 0.3.1 | MIT | https://crates.io/crates/want |
| wasi | 0.11.1+wasi-snapshot-preview1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | https://crates.io/crates/wasi |
| wasip2 | 1.0.4+wasi-0.2.12 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | https://crates.io/crates/wasip2 |
| wasm-bindgen | 0.2.129 | MIT OR Apache-2.0 | https://crates.io/crates/wasm-bindgen |
| wasm-bindgen-futures | 0.4.79 | MIT OR Apache-2.0 | https://crates.io/crates/wasm-bindgen-futures |
| wasm-bindgen-macro | 0.2.129 | MIT OR Apache-2.0 | https://crates.io/crates/wasm-bindgen-macro |
| wasm-bindgen-macro-support | 0.2.129 | MIT OR Apache-2.0 | https://crates.io/crates/wasm-bindgen-macro-support |
| wasm-bindgen-shared | 0.2.129 | MIT OR Apache-2.0 | https://crates.io/crates/wasm-bindgen-shared |
| wasm-bindgen-test | 0.3.79 | MIT OR Apache-2.0 | https://crates.io/crates/wasm-bindgen-test |
| wasm-bindgen-test-macro | 0.3.79 | MIT OR Apache-2.0 | https://crates.io/crates/wasm-bindgen-test-macro |
| wasm-bindgen-test-shared | 0.2.129 | MIT OR Apache-2.0 | https://crates.io/crates/wasm-bindgen-test-shared |
| wasm-streams | 0.5.0 | MIT OR Apache-2.0 | https://crates.io/crates/wasm-streams |
| web-sys | 0.3.106 | MIT OR Apache-2.0 | https://crates.io/crates/web-sys |
| web-time | 1.1.0 | MIT OR Apache-2.0 | https://crates.io/crates/web-time |
| web_atoms | 0.2.6 | MIT OR Apache-2.0 | https://crates.io/crates/web_atoms |
| web_atoms | 0.3.0 | MIT OR Apache-2.0 | https://crates.io/crates/web_atoms |
| webkit2gtk | 2.0.2 | MIT | https://crates.io/crates/webkit2gtk |
| webkit2gtk-sys | 2.0.2 | MIT | https://crates.io/crates/webkit2gtk-sys |
| webpki-root-certs | 1.0.9 | CDLA-Permissive-2.0 | https://crates.io/crates/webpki-root-certs |
| webpki-roots | 1.0.9 | CDLA-Permissive-2.0 | https://crates.io/crates/webpki-roots |
| webview2-com | 0.39.1 | MIT | https://crates.io/crates/webview2-com |
| webview2-com-macros | 0.8.1 | MIT | https://crates.io/crates/webview2-com-macros |
| webview2-com-sys | 0.39.1 | MIT | https://crates.io/crates/webview2-com-sys |
| weezl | 0.1.12 | MIT OR Apache-2.0 | https://crates.io/crates/weezl |
| whoami | 2.1.3 | Apache-2.0 OR BSL-1.0 OR MIT | https://crates.io/crates/whoami |
| wide | 1.7.1 | Zlib OR Apache-2.0 OR MIT | https://crates.io/crates/wide |
| widestring | 1.2.1 | MIT OR Apache-2.0 | https://crates.io/crates/widestring |
| winapi | 0.3.9 | MIT/Apache-2.0 | https://crates.io/crates/winapi |
| winapi-i686-pc-windows-gnu | 0.4.0 | MIT/Apache-2.0 | https://crates.io/crates/winapi-i686-pc-windows-gnu |
| winapi-util | 0.1.11 | Unlicense OR MIT | https://crates.io/crates/winapi-util |
| winapi-x86_64-pc-windows-gnu | 0.4.0 | MIT/Apache-2.0 | https://crates.io/crates/winapi-x86_64-pc-windows-gnu |
| window-vibrancy | 0.8.1 | Apache-2.0 OR MIT | https://crates.io/crates/window-vibrancy |
| windows | 0.62.2 | MIT OR Apache-2.0 | https://crates.io/crates/windows |
| windows-collections | 0.3.2 | MIT OR Apache-2.0 | https://crates.io/crates/windows-collections |
| windows-core | 0.62.2 | MIT OR Apache-2.0 | https://crates.io/crates/windows-core |
| windows-future | 0.3.2 | MIT OR Apache-2.0 | https://crates.io/crates/windows-future |
| windows-implement | 0.60.2 | MIT OR Apache-2.0 | https://crates.io/crates/windows-implement |
| windows-interface | 0.59.3 | MIT OR Apache-2.0 | https://crates.io/crates/windows-interface |
| windows-link | 0.2.1 | MIT OR Apache-2.0 | https://crates.io/crates/windows-link |
| windows-native-keyring-store | 1.1.0 | MIT OR Apache-2.0 | https://crates.io/crates/windows-native-keyring-store |
| windows-numerics | 0.3.1 | MIT OR Apache-2.0 | https://crates.io/crates/windows-numerics |
| windows-registry | 0.6.1 | MIT OR Apache-2.0 | https://crates.io/crates/windows-registry |
| windows-result | 0.4.1 | MIT OR Apache-2.0 | https://crates.io/crates/windows-result |
| windows-strings | 0.5.1 | MIT OR Apache-2.0 | https://crates.io/crates/windows-strings |
| windows-sys | 0.45.0 | MIT OR Apache-2.0 | https://crates.io/crates/windows-sys |
| windows-sys | 0.52.0 | MIT OR Apache-2.0 | https://crates.io/crates/windows-sys |
| windows-sys | 0.59.0 | MIT OR Apache-2.0 | https://crates.io/crates/windows-sys |
| windows-sys | 0.60.2 | MIT OR Apache-2.0 | https://crates.io/crates/windows-sys |
| windows-sys | 0.61.2 | MIT OR Apache-2.0 | https://crates.io/crates/windows-sys |
| windows-targets | 0.42.2 | MIT OR Apache-2.0 | https://crates.io/crates/windows-targets |
| windows-targets | 0.52.6 | MIT OR Apache-2.0 | https://crates.io/crates/windows-targets |
| windows-targets | 0.53.5 | MIT OR Apache-2.0 | https://crates.io/crates/windows-targets |
| windows-threading | 0.2.1 | MIT OR Apache-2.0 | https://crates.io/crates/windows-threading |
| windows-version | 0.1.7 | MIT OR Apache-2.0 | https://crates.io/crates/windows-version |
| windows_aarch64_gnullvm | 0.42.2 | MIT OR Apache-2.0 | https://crates.io/crates/windows_aarch64_gnullvm |
| windows_aarch64_gnullvm | 0.52.6 | MIT OR Apache-2.0 | https://crates.io/crates/windows_aarch64_gnullvm |
| windows_aarch64_gnullvm | 0.53.1 | MIT OR Apache-2.0 | https://crates.io/crates/windows_aarch64_gnullvm |
| windows_aarch64_msvc | 0.42.2 | MIT OR Apache-2.0 | https://crates.io/crates/windows_aarch64_msvc |
| windows_aarch64_msvc | 0.52.6 | MIT OR Apache-2.0 | https://crates.io/crates/windows_aarch64_msvc |
| windows_aarch64_msvc | 0.53.1 | MIT OR Apache-2.0 | https://crates.io/crates/windows_aarch64_msvc |
| windows_i686_gnu | 0.42.2 | MIT OR Apache-2.0 | https://crates.io/crates/windows_i686_gnu |
| windows_i686_gnu | 0.52.6 | MIT OR Apache-2.0 | https://crates.io/crates/windows_i686_gnu |
| windows_i686_gnu | 0.53.1 | MIT OR Apache-2.0 | https://crates.io/crates/windows_i686_gnu |
| windows_i686_gnullvm | 0.52.6 | MIT OR Apache-2.0 | https://crates.io/crates/windows_i686_gnullvm |
| windows_i686_gnullvm | 0.53.1 | MIT OR Apache-2.0 | https://crates.io/crates/windows_i686_gnullvm |
| windows_i686_msvc | 0.42.2 | MIT OR Apache-2.0 | https://crates.io/crates/windows_i686_msvc |
| windows_i686_msvc | 0.52.6 | MIT OR Apache-2.0 | https://crates.io/crates/windows_i686_msvc |
| windows_i686_msvc | 0.53.1 | MIT OR Apache-2.0 | https://crates.io/crates/windows_i686_msvc |
| windows_x86_64_gnu | 0.42.2 | MIT OR Apache-2.0 | https://crates.io/crates/windows_x86_64_gnu |
| windows_x86_64_gnu | 0.52.6 | MIT OR Apache-2.0 | https://crates.io/crates/windows_x86_64_gnu |
| windows_x86_64_gnu | 0.53.1 | MIT OR Apache-2.0 | https://crates.io/crates/windows_x86_64_gnu |
| windows_x86_64_gnullvm | 0.42.2 | MIT OR Apache-2.0 | https://crates.io/crates/windows_x86_64_gnullvm |
| windows_x86_64_gnullvm | 0.52.6 | MIT OR Apache-2.0 | https://crates.io/crates/windows_x86_64_gnullvm |
| windows_x86_64_gnullvm | 0.53.1 | MIT OR Apache-2.0 | https://crates.io/crates/windows_x86_64_gnullvm |
| windows_x86_64_msvc | 0.42.2 | MIT OR Apache-2.0 | https://crates.io/crates/windows_x86_64_msvc |
| windows_x86_64_msvc | 0.52.6 | MIT OR Apache-2.0 | https://crates.io/crates/windows_x86_64_msvc |
| windows_x86_64_msvc | 0.53.1 | MIT OR Apache-2.0 | https://crates.io/crates/windows_x86_64_msvc |
| winnow | 0.5.40 | MIT | https://crates.io/crates/winnow |
| winnow | 1.0.4 | MIT | https://crates.io/crates/winnow |
| winreg | 0.55.0 | MIT | https://crates.io/crates/winreg |
| wiremock | 0.6.5 | MIT/Apache-2.0 | https://crates.io/crates/wiremock |
| wit-bindgen | 0.57.1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | https://crates.io/crates/wit-bindgen |
| wmi | 0.18.4 | MIT OR Apache-2.0 | https://crates.io/crates/wmi |
| writeable | 0.6.4 | Unicode-3.0 | https://crates.io/crates/writeable |
| wry | 0.57.0 | Apache-2.0 OR MIT | https://crates.io/crates/wry |
| ws_stream_wasm | 0.7.5 | Unlicense | https://crates.io/crates/ws_stream_wasm |
| x11 | 2.21.0 | MIT | https://crates.io/crates/x11 |
| x11-dl | 2.21.0 | MIT | https://crates.io/crates/x11-dl |
| x25519-dalek | 2.0.1 | BSD-3-Clause | https://crates.io/crates/x25519-dalek |
| x509-cert | 0.2.5 | Apache-2.0 OR MIT | https://crates.io/crates/x509-cert |
| x509-parser | 0.18.1 | MIT OR Apache-2.0 | https://crates.io/crates/x509-parser |
| xattr | 1.6.1 | MIT OR Apache-2.0 | https://crates.io/crates/xattr |
| xml-rs | 0.8.29 | MIT | https://crates.io/crates/xml-rs |
| xmltree | 0.10.3 | MIT | https://crates.io/crates/xmltree |
| xxhash-rust | 0.8.19 | BSL-1.0 | https://crates.io/crates/xxhash-rust |
| yasna | 0.6.0 | MIT OR Apache-2.0 | https://crates.io/crates/yasna |
| yoke | 0.8.3 | Unicode-3.0 | https://crates.io/crates/yoke |
| yoke-derive | 0.8.3 | Unicode-3.0 | https://crates.io/crates/yoke-derive |
| z32 | 1.3.0 | MIT | https://crates.io/crates/z32 |
| zbus | 5.19.0 | MIT | https://crates.io/crates/zbus |
| zbus-secret-service-keyring-store | 1.0.1 | MIT OR Apache-2.0 | https://crates.io/crates/zbus-secret-service-keyring-store |
| zbus_macros | 5.19.0 | MIT | https://crates.io/crates/zbus_macros |
| zbus_names | 4.3.4 | MIT | https://crates.io/crates/zbus_names |
| zcheapstr | 1.1.0 | MIT | https://crates.io/crates/zcheapstr |
| zerocopy | 0.8.59 | BSD-2-Clause OR Apache-2.0 OR MIT | https://crates.io/crates/zerocopy |
| zerocopy-derive | 0.8.59 | BSD-2-Clause OR Apache-2.0 OR MIT | https://crates.io/crates/zerocopy-derive |
| zerofrom | 0.1.8 | Unicode-3.0 | https://crates.io/crates/zerofrom |
| zerofrom-derive | 0.1.8 | Unicode-3.0 | https://crates.io/crates/zerofrom-derive |
| zeroize | 1.9.0 | Apache-2.0 OR MIT | https://crates.io/crates/zeroize |
| zeroize_derive | 1.5.0 | Apache-2.0 OR MIT | https://crates.io/crates/zeroize_derive |
| zerotrie | 0.2.5 | Unicode-3.0 | https://crates.io/crates/zerotrie |
| zerovec | 0.11.8 | Unicode-3.0 | https://crates.io/crates/zerovec |
| zerovec-derive | 0.11.6 | Unicode-3.0 | https://crates.io/crates/zerovec-derive |
| zip | 4.6.1 | MIT | https://crates.io/crates/zip |
| zlib-rs | 0.6.8 | Zlib | https://crates.io/crates/zlib-rs |
| zmij | 1.0.23 | MIT | https://crates.io/crates/zmij |
| zstd | 0.13.3 | MIT | https://crates.io/crates/zstd |
| zstd-safe | 7.3.0 | BSD-3-Clause | https://crates.io/crates/zstd-safe |
| zstd-sys | 2.1.1+zstd.1.5.7 | BSD-3-Clause | https://crates.io/crates/zstd-sys |
| zune-core | 0.5.3 | MIT OR Apache-2.0 OR Zlib | https://crates.io/crates/zune-core |
| zune-jpeg | 0.5.15 | MIT OR Apache-2.0 OR Zlib | https://crates.io/crates/zune-jpeg |
| zvariant | 5.15.0 | MIT | https://crates.io/crates/zvariant |
| zvariant_derive | 5.15.0 | MIT | https://crates.io/crates/zvariant_derive |
| zvariant_utils | 4.2.0 | MIT | https://crates.io/crates/zvariant_utils |

## Desktop npm packages

Total packages: 138

| Package | Version | License | Source |
| --- | --- | --- | --- |
| @babel/runtime | 7.29.7 | MIT | https://babel.dev/docs/en/next/babel-runtime |
| @dimforge/rapier3d-compat | 0.12.0 | Apache-2.0 | https://rapier.rs |
| @floating-ui/core | 1.8.0 | MIT | https://floating-ui.com |
| @floating-ui/dom | 1.8.0 | MIT | https://floating-ui.com |
| @floating-ui/react-dom | 2.1.9 | MIT | https://floating-ui.com/docs/react-dom |
| @floating-ui/utils | 0.2.12 | MIT | https://floating-ui.com |
| @mediapipe/tasks-vision | 0.10.17 | Apache-2.0 | http://mediapipe.dev |
| @monogrid/gainmap-js | 3.4.0 | MIT | https://github.com/MONOGRID/gainmap-js#readme |
| @pixiv/three-vrm | 3.5.5 | MIT | https://github.com/pixiv/three-vrm#readme |
| @pixiv/three-vrm-animation | 3.5.5 | MIT | https://github.com/pixiv/three-vrm#readme |
| @pixiv/three-vrm-core | 3.5.5 | MIT | https://github.com/pixiv/three-vrm#readme |
| @pixiv/three-vrm-materials-hdr-emissive-multiplier | 3.5.5 | MIT | https://github.com/pixiv/three-vrm#readme |
| @pixiv/three-vrm-materials-mtoon | 3.5.5 | MIT | https://github.com/pixiv/three-vrm#readme |
| @pixiv/three-vrm-materials-v0compat | 3.5.5 | MIT | https://github.com/pixiv/three-vrm#readme |
| @pixiv/three-vrm-node-constraint | 3.5.5 | MIT | https://github.com/pixiv/three-vrm#readme |
| @pixiv/three-vrm-springbone | 3.5.5 | MIT | https://github.com/pixiv/three-vrm#readme |
| @pixiv/types-vrm-0.0 | 3.5.5 | MIT | https://github.com/pixiv/three-vrm#readme |
| @pixiv/types-vrmc-materials-hdr-emissive-multiplier-1.0 | 3.5.5 | MIT | https://github.com/pixiv/three-vrm#readme |
| @pixiv/types-vrmc-materials-mtoon-1.0 | 3.5.5 | MIT | https://github.com/pixiv/three-vrm#readme |
| @pixiv/types-vrmc-node-constraint-1.0 | 3.5.5 | MIT | https://github.com/pixiv/three-vrm#readme |
| @pixiv/types-vrmc-springbone-1.0 | 3.5.5 | MIT | https://github.com/pixiv/three-vrm#readme |
| @pixiv/types-vrmc-springbone-extended-collider-1.0 | 3.5.5 | MIT | https://github.com/pixiv/three-vrm#readme |
| @pixiv/types-vrmc-vrm-1.0 | 3.5.5 | MIT | https://github.com/pixiv/three-vrm#readme |
| @pixiv/types-vrmc-vrm-animation-1.0 | 3.5.5 | MIT | https://github.com/pixiv/three-vrm#readme |
| @radix-ui/primitive | 1.1.7 | MIT | https://radix-ui.com/primitives |
| @radix-ui/react-arrow | 1.1.15 | MIT | https://radix-ui.com/primitives |
| @radix-ui/react-compose-refs | 1.1.5 | MIT | https://radix-ui.com/primitives |
| @radix-ui/react-context | 1.2.2 | MIT | https://radix-ui.com/primitives |
| @radix-ui/react-dialog | 1.1.23 | MIT | https://radix-ui.com/primitives |
| @radix-ui/react-dismissable-layer | 1.1.19 | MIT | https://radix-ui.com/primitives |
| @radix-ui/react-focus-guards | 1.1.6 | MIT | https://radix-ui.com/primitives |
| @radix-ui/react-focus-scope | 1.1.16 | MIT | https://radix-ui.com/primitives |
| @radix-ui/react-hover-card | 1.1.23 | MIT | https://radix-ui.com/primitives |
| @radix-ui/react-id | 1.1.4 | MIT | https://radix-ui.com/primitives |
| @radix-ui/react-popover | 1.1.23 | MIT | https://radix-ui.com/primitives |
| @radix-ui/react-popper | 1.3.7 | MIT | https://radix-ui.com/primitives |
| @radix-ui/react-portal | 1.1.17 | MIT | https://radix-ui.com/primitives |
| @radix-ui/react-presence | 1.1.10 | MIT | https://radix-ui.com/primitives |
| @radix-ui/react-primitive | 2.1.10 | MIT | https://radix-ui.com/primitives |
| @radix-ui/react-slot | 1.3.3 | MIT | https://radix-ui.com/primitives |
| @radix-ui/react-tooltip | 1.2.16 | MIT | https://radix-ui.com/primitives |
| @radix-ui/react-use-callback-ref | 1.1.4 | MIT | https://radix-ui.com/primitives |
| @radix-ui/react-use-controllable-state | 1.2.6 | MIT | https://radix-ui.com/primitives |
| @radix-ui/react-use-effect-event | 0.0.5 | MIT | https://radix-ui.com/primitives |
| @radix-ui/react-use-layout-effect | 1.1.4 | MIT | https://radix-ui.com/primitives |
| @radix-ui/react-use-rect | 1.1.4 | MIT | https://radix-ui.com/primitives |
| @radix-ui/react-use-size | 1.1.4 | MIT | https://radix-ui.com/primitives |
| @radix-ui/react-visually-hidden | 1.2.11 | MIT | https://radix-ui.com/primitives |
| @radix-ui/rect | 1.1.3 | MIT | https://radix-ui.com/primitives |
| @react-three/drei | 10.7.9 | MIT | https://github.com/pmndrs/drei |
| @react-three/fiber | 9.8.1 | MIT | https://github.com/pmndrs/react-three-fiber#readme |
| @tauri-apps/api | 2.12.0 | Apache-2.0 OR MIT | https://github.com/tauri-apps/tauri#readme |
| @tauri-apps/plugin-deep-link | 2.6.0 | MIT OR Apache-2.0 | https://github.com/tauri-apps/plugins-workspace#readme |
| @tauri-apps/plugin-dialog | 2.8.0 | MIT OR Apache-2.0 | https://github.com/tauri-apps/plugins-workspace#readme |
| @tauri-apps/plugin-updater | 2.13.1 | MIT OR Apache-2.0 | https://github.com/tauri-apps/plugins-workspace#readme |
| @tweenjs/tween.js | 23.1.3 | MIT | https://github.com/tweenjs/tween.js |
| @types/draco3d | 1.4.10 | MIT | https://github.com/DefinitelyTyped/DefinitelyTyped/tree/master/types/draco3d |
| @types/offscreencanvas | 2019.7.3 | MIT | https://github.com/DefinitelyTyped/DefinitelyTyped/tree/master/types/offscreencanvas |
| @types/react | 19.3.0 | MIT | https://github.com/DefinitelyTyped/DefinitelyTyped/tree/master/types/react |
| @types/react-dom | 19.3.0 | MIT | https://github.com/DefinitelyTyped/DefinitelyTyped/tree/master/types/react-dom |
| @types/react-reconciler | 0.28.9 | MIT | https://github.com/DefinitelyTyped/DefinitelyTyped/tree/master/types/react-reconciler |
| @types/stats.js | 0.17.4 | MIT | https://github.com/DefinitelyTyped/DefinitelyTyped/tree/master/types/stats.js |
| @types/three | 0.186.0 | MIT | https://github.com/DefinitelyTyped/DefinitelyTyped/tree/master/types/three |
| @types/webxr | 0.5.24 | MIT | https://github.com/DefinitelyTyped/DefinitelyTyped/tree/master/types/webxr |
| @use-gesture/core | 10.3.1 | MIT | https://use-gesture.netlify.app |
| @use-gesture/react | 10.3.1 | MIT | https://use-gesture.netlify.app |
| aria-hidden | 1.2.6 | MIT | https://github.com/theKashey/aria-hidden#readme |
| base64-js | 1.5.1 | MIT | https://github.com/beatgammit/base64-js |
| bidi-js | 1.1.0 | MIT | https://github.com/lojjic/bidi-js#readme |
| buffer | 6.0.3 | MIT | https://github.com/feross/buffer |
| camera-controls | 3.1.2 | MIT | https://github.com/yomotsu/camera-controls#readme |
| class-variance-authority | 0.7.1 | Apache-2.0 | https://github.com/joe-bell/cva#readme |
| clsx | 2.1.1 | MIT | https://github.com/lukeed/clsx#readme |
| cookie | 1.1.1 | MIT | https://github.com/jshttp/cookie#readme |
| cross-env | 7.0.3 | MIT | https://github.com/kentcdodds/cross-env#readme |
| cross-spawn | 7.0.6 | MIT | https://github.com/moxystudio/node-cross-spawn |
| csstype | 3.2.3 | MIT | https://github.com/frenic/csstype#readme |
| detect-gpu | 5.0.70 | MIT | https://github.com/pmndrs/detect-gpu#readme |
| detect-node-es | 1.1.0 | MIT | https://github.com/thekashey/detect-node |
| draco3d | 1.5.7 | Apache-2.0 | https://github.com/google/draco#readme |
| fflate | 0.6.11 | MIT | https://101arrowz.github.io/fflate |
| fflate | 0.8.3 | MIT | https://101arrowz.github.io/fflate |
| get-nonce | 1.0.1 | MIT | https://github.com/theKashey/get-nonce |
| glsl-noise | 0.0.0 | MIT | https://github.com/hughsk/glsl-noise#readme |
| hls.js | 1.7.3 | Apache-2.0 | https://github.com/video-dev/hls.js |
| html-parse-stringify | 4.0.1 | MIT | https://github.com/i18next/html-parse-stringify |
| i18next | 26.4.2 | MIT | https://www.i18next.com |
| ieee754 | 1.2.1 | BSD-3-Clause | https://github.com/feross/ieee754#readme |
| immediate | 3.0.6 | MIT | https://github.com/calvinmetcalf/immediate#readme |
| is-promise | 2.2.2 | MIT | https://github.com/then/is-promise#readme |
| isexe | 2.0.0 | ISC | https://github.com/isaacs/isexe#readme |
| its-fine | 2.1.1 | MIT | https://github.com/pmndrs/its-fine |
| jsqr | 1.4.0 | Apache-2.0 | https://github.com/cozmo/jsQR#readme |
| lie | 3.3.0 | MIT | https://github.com/calvinmetcalf/lie#readme |
| lucide-react | 1.49.0 | ISC | https://lucide.dev |
| maath | 0.10.8 | MIT | - |
| meshline | 3.3.1 | MIT | https://github.com/pmndrs/meshline#readme |
| meshoptimizer | 1.1.1 | MIT | https://github.com/zeux/meshoptimizer |
| path-key | 3.1.1 | MIT | https://github.com/sindresorhus/path-key#readme |
| potpack | 1.0.2 | ISC | https://mapbox.github.io/potpack/ |
| promise-worker-transferable | 1.0.4 | Apache-2.0 | https://github.com/terikon/promise-worker-transferable#readme |
| react | 19.3.0 | MIT | https://react.dev/ |
| react-dom | 19.3.0 | MIT | https://react.dev/ |
| react-i18next | 17.0.15 | MIT | https://github.com/i18next/react-i18next |
| react-remove-scroll | 2.7.2 | MIT | https://github.com/theKashey/react-remove-scroll#readme |
| react-remove-scroll-bar | 2.3.8 | MIT | https://github.com/theKashey/react-remove-scroll-bar#readme |
| react-router | 7.18.4 | MIT | https://github.com/remix-run/react-router#readme |
| react-router-dom | 7.18.4 | MIT | https://github.com/remix-run/react-router#readme |
| react-style-singleton | 2.2.3 | MIT | https://github.com/theKashey/react-style-singleton#readme |
| react-use-measure | 2.1.7 | MIT | https://github.com/pmndrs/react-use-measure |
| require-from-string | 2.0.2 | MIT | https://github.com/floatdrop/require-from-string#readme |
| scheduler | 0.28.0 | MIT | https://react.dev/ |
| set-cookie-parser | 2.7.2 | MIT | https://github.com/nfriedly/set-cookie-parser |
| shebang-command | 2.0.0 | MIT | https://github.com/kevva/shebang-command#readme |
| shebang-regex | 3.0.0 | MIT | https://github.com/sindresorhus/shebang-regex#readme |
| stats-gl | 2.4.2 | MIT | https://github.com/RenaudRohlinger/stats-gl |
| stats.js | 0.17.0 | MIT | https://github.com/mrdoob/stats.js |
| suspend-react | 0.1.3 | MIT | https://github.com/pmndrs/suspend-react#readme |
| tailwind-merge | 3.7.0 | MIT | https://github.com/dcastil/tailwind-merge |
| three | 0.186.1 | MIT | https://threejs.org/ |
| three-mesh-bvh | 0.8.3 | MIT | https://github.com/gkjohnson/three-mesh-bvh#readme |
| three-stdlib | 2.36.1 | MIT | https://github.com/pmndrs/three-stdlib |
| troika-three-text | 0.52.5 | MIT | https://github.com/protectwise/troika#readme |
| troika-three-utils | 0.52.5 | MIT | https://github.com/protectwise/troika#readme |
| troika-worker-utils | 0.52.0 | MIT | https://github.com/protectwise/troika#readme |
| tslib | 2.8.1 | 0BSD | https://www.typescriptlang.org/ |
| tunnel-rat | 0.1.2 | MIT | https://github.com/pmndrs/tunnel-rat#readme |
| typescript | 6.0.3 | Apache-2.0 | https://www.typescriptlang.org/ |
| uqr | 0.1.3 | MIT | https://github.com/unjs/uqr#readme |
| use-callback-ref | 1.3.3 | MIT | https://github.com/theKashey/use-callback-ref#readme |
| use-sidecar | 1.1.3 | MIT | https://github.com/theKashey/use-sidecar |
| use-sync-external-store | 1.7.0 | MIT | https://github.com/react/react#readme |
| utility-types | 3.11.0 | MIT | https://github.com/piotrwitek/utility-types |
| webgl-constants | 1.1.1 | MIT | - |
| webgl-sdf-generator | 1.1.1 | MIT | https://github.com/lojjic/webgl-sdf-generator#readme |
| which | 2.0.2 | ISC | https://github.com/isaacs/node-which#readme |
| zustand | 4.5.7 | MIT | https://github.com/pmndrs/zustand |
| zustand | 5.0.15 | MIT | https://github.com/pmndrs/zustand |
