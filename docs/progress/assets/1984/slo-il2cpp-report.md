# Game SDK の性能の計測（ADR 0070 §8.4）

Backend: IL2CPP / Runs: 5 × 180 s / memory: 600 s / commit: 9488f0dd392ffb2a63b3ddc47d9e66f594ab3e71

| 項目 | run 1 | run 2 | run 3 | run 4 | run 5 | 判定 |
| --- | --- | --- | --- | --- | --- | --- |
| frame time p95 ≤ 16.7 ms | ○ | ○ | ○ | ○ | ○ | PASS |
| frame time p99 ≤ 25 ms | ○ | ○ | ○ | ○ | ○ | PASS |
| 入力の確定 p95 ≤ 100 ms | ○ | ○ | ○ | ○ | ○ | PASS |
| 映像遅延 p95 直接 ≤ 500 ms | ○ | ○ | ○ | ○ | ○ | PASS |
| 映像遅延 p95 転送 1 段 ≤ 800 ms | ○ | ○ | ○ | ○ | ○ | PASS |
| 適用中の frame time の最大 ≤ 33 ms | ○ | ○ | ○ | ○ | ○ | PASS |
| 対戦 ≤ 200 kbps / 人 | ○ | ○ | ○ | ○ | ○ | PASS |
| 配信者の上り ≤ 10 Mbps | ○ | ○ | ○ | ○ | ○ | PASS |
| 復帰 対戦 ≤ 5 s | ○ | ○ | ○ | ○ | ○ | PASS |
| 復帰 視聴 ≤ 20 s | ○ | ○ | ○ | ○ | ○ | PASS |
| SDK の memory ≤ 256 MiB | ○ | ○ | ○ | ○ | ○ | PASS |
| poll p99 ≤ 1 ms | ○ | ○ | ○ | ○ | ○ | PASS |
| asset の反映 p95 ≤ 10 s（5 回 × 2 人） | 3.2/2.3 | 4.3/2.4 | 3.7/2.8 | 4.9/2.3 | 5.4/3.2 | PASS (5.4 s) |
| memory の増加 ≤ 5%（600 秒、開始 1 分後から） | host -3.8%, client 0.7%, viewer1 1.1%, viewer2 -1.6%, viewer3 -2.3%, viewer4 0.2%, viewer5 1.5% | PASS |

経路: relay と DHT を使わないので、送信量はすべて Direct P2P（Relay Supported P2P・Relay Fallback は 0）。

## 値

### run 1
- frame_p95_host: 9.35
- frame_p99_host: 9.55
- input_p95_host: 0.04
- input_count_host: 13
- frame_p95_client: 8.37
- frame_p99_client: 8.42
- input_p95_client: 83.45
- input_count_client: 5
- video_p95_viewer1: 140.10
- video_hops_viewer1: 2
- video_p95_viewer2: 121.38
- video_hops_viewer2: 1
- video_p95_viewer3: 110.61
- video_hops_viewer3: 1
- video_p95_viewer4: 139.50
- video_hops_viewer4: 1
- video_p95_viewer5: 114.10
- video_hops_viewer5: 1
- video_p95_direct: 116.19
- video_p95_relayed: 140.10
- asset_s_host: 3.21
- asset_publish_s: 0.04
- asset_hint_s_host: 2.96
- asset_frame_max_host: 12.63
- asset_s_client: 2.27
- asset_hint_s_client: 2.02
- asset_frame_max_client: 8.36
- match_kbps_host: 52.90
- match_kbps_client: 42.80
- broadcaster_mbps: 6.61
- unmapped_pairs: 0
- publisher_upload_mbps: 0.56
- recover_match_s: 0.04
- recover_video_s: 5.62
- memory_mib_host: 19.46
- poll_p99_host: 0.02
- memory_mib_client: 10.04
- poll_p99_client: 0.02
- memory_mib_viewer1: 15.79
- poll_p99_viewer1: 0.34
- memory_mib_viewer2: 15.90
- poll_p99_viewer2: 0.35
- memory_mib_viewer3: 16.23
- poll_p99_viewer3: 0.36
- memory_mib_viewer4: 16.00
- poll_p99_viewer4: 0.34
- memory_mib_viewer5: 16.49
- poll_p99_viewer5: 0.37

### run 2
- frame_p95_host: 9.26
- frame_p99_host: 9.47
- input_p95_host: 0.03
- input_count_host: 2
- frame_p95_client: 8.39
- frame_p99_client: 8.51
- input_p95_client: 83.38
- input_count_client: 3
- video_p95_viewer1: 181.26
- video_hops_viewer1: 2
- video_p95_viewer2: 106.83
- video_hops_viewer2: 1
- video_p95_viewer3: 79.88
- video_hops_viewer3: 1
- video_p95_viewer4: 82.25
- video_hops_viewer4: 1
- video_p95_viewer5: 123.12
- video_hops_viewer5: 1
- video_p95_direct: 98.18
- video_p95_relayed: 181.26
- asset_s_host: 4.26
- asset_publish_s: 0.04
- asset_hint_s_host: 3.74
- asset_frame_max_host: 9.26
- asset_s_client: 2.42
- asset_hint_s_client: 2.11
- asset_frame_max_client: 8.58
- match_kbps_host: 53.60
- match_kbps_client: 44.16
- broadcaster_mbps: 6.62
- unmapped_pairs: 0
- publisher_upload_mbps: 0.54
- recover_match_s: 0.04
- recover_video_s: 4.86
- memory_mib_host: 18.85
- poll_p99_host: 0.02
- memory_mib_client: 9.68
- poll_p99_client: 0.02
- memory_mib_viewer1: 15.75
- poll_p99_viewer1: 0.30
- memory_mib_viewer2: 15.92
- poll_p99_viewer2: 0.33
- memory_mib_viewer3: 14.60
- poll_p99_viewer3: 0.34
- memory_mib_viewer4: 14.57
- poll_p99_viewer4: 0.35
- memory_mib_viewer5: 14.90
- poll_p99_viewer5: 0.33

### run 3
- frame_p95_host: 9.08
- frame_p99_host: 9.24
- input_p95_host: 0.06
- input_count_host: 9
- frame_p95_client: 8.39
- frame_p99_client: 8.48
- input_p95_client: 83.33
- input_count_client: 6
- video_p95_viewer1: 108.00
- video_hops_viewer1: 1
- video_p95_viewer2: 112.48
- video_hops_viewer2: 1
- video_p95_viewer3: 111.68
- video_hops_viewer3: 1
- video_p95_viewer4: 139.08
- video_hops_viewer4: 2
- video_p95_viewer5: 110.77
- video_hops_viewer5: 1
- video_p95_direct: 111.85
- video_p95_relayed: 139.08
- asset_s_host: 3.67
- asset_publish_s: 0.04
- asset_hint_s_host: 2.08
- asset_frame_max_host: 10.46
- asset_s_client: 2.80
- asset_hint_s_client: 2.45
- asset_frame_max_client: 10.13
- match_kbps_host: 54.43
- match_kbps_client: 44.58
- broadcaster_mbps: 6.62
- unmapped_pairs: 0
- publisher_upload_mbps: 0.58
- recover_match_s: 0.06
- recover_video_s: 6.30
- memory_mib_host: 18.72
- poll_p99_host: 0.02
- memory_mib_client: 10.22
- poll_p99_client: 0.03
- memory_mib_viewer1: 15.81
- poll_p99_viewer1: 0.27
- memory_mib_viewer2: 15.94
- poll_p99_viewer2: 0.29
- memory_mib_viewer3: 16.14
- poll_p99_viewer3: 0.26
- memory_mib_viewer4: 15.84
- poll_p99_viewer4: 0.23
- memory_mib_viewer5: 16.44
- poll_p99_viewer5: 0.28

### run 4
- frame_p95_host: 9.14
- frame_p99_host: 9.30
- input_p95_host: 0.03
- input_count_host: 13
- frame_p95_client: 8.38
- frame_p99_client: 8.48
- input_p95_client: 83.36
- input_count_client: 3
- video_p95_viewer1: 84.04
- video_hops_viewer1: 1
- video_p95_viewer2: 139.31
- video_hops_viewer2: 2
- video_p95_viewer3: 111.12
- video_hops_viewer3: 1
- video_p95_viewer4: 111.28
- video_hops_viewer4: 1
- video_p95_viewer5: 77.42
- video_hops_viewer5: 1
- video_p95_direct: 109.56
- video_p95_relayed: 139.31
- asset_s_host: 4.88
- asset_publish_s: 0.04
- asset_hint_s_host: 3.58
- asset_frame_max_host: 9.51
- asset_s_client: 2.33
- asset_hint_s_client: 2.08
- asset_frame_max_client: 8.46
- match_kbps_host: 53.56
- match_kbps_client: 43.70
- broadcaster_mbps: 6.58
- unmapped_pairs: 0
- publisher_upload_mbps: 0.56
- recover_match_s: 0.06
- recover_video_s: 1.32
- memory_mib_host: 19.03
- poll_p99_host: 0.02
- memory_mib_client: 9.78
- poll_p99_client: 0.02
- memory_mib_viewer1: 14.66
- poll_p99_viewer1: 0.26
- memory_mib_viewer2: 16.23
- poll_p99_viewer2: 0.25
- memory_mib_viewer3: 16.44
- poll_p99_viewer3: 0.25
- memory_mib_viewer4: 15.92
- poll_p99_viewer4: 0.29
- memory_mib_viewer5: 14.44
- poll_p99_viewer5: 0.25

### run 5
- frame_p95_host: 9.07
- frame_p99_host: 9.22
- input_p95_host: 0.04
- input_count_host: 17
- frame_p95_client: 8.38
- frame_p99_client: 8.49
- input_p95_client: 83.38
- input_count_client: 8
- video_p95_viewer1: 89.20
- video_hops_viewer1: 1
- video_p95_viewer2: 246.87
- video_hops_viewer2: 1
- video_p95_viewer3: 137.29
- video_hops_viewer3: 2
- video_p95_viewer4: 99.62
- video_hops_viewer4: 1
- video_p95_viewer5: 111.73
- video_hops_viewer5: 1
- video_p95_direct: 113.54
- video_p95_relayed: 137.29
- asset_s_host: 5.41
- asset_publish_s: 0.04
- asset_hint_s_host: 4.04
- asset_frame_max_host: 9.39
- asset_s_client: 3.17
- asset_hint_s_client: 2.66
- asset_frame_max_client: 9.16
- match_kbps_host: 54.67
- match_kbps_client: 44.53
- broadcaster_mbps: 6.62
- unmapped_pairs: 0
- publisher_upload_mbps: 0.52
- recover_match_s: 0.04
- recover_video_s: 4.70
- memory_mib_host: 19.45
- poll_p99_host: 0.02
- memory_mib_client: 9.44
- poll_p99_client: 0.02
- memory_mib_viewer1: 16.17
- poll_p99_viewer1: 0.25
- memory_mib_viewer2: 24.64
- poll_p99_viewer2: 0.24
- memory_mib_viewer3: 15.94
- poll_p99_viewer3: 0.26
- memory_mib_viewer4: 15.82
- poll_p99_viewer4: 0.26
- memory_mib_viewer5: 15.50
- poll_p99_viewer5: 0.27
