# M9 export measurements

From [EXPORT.md](../EXPORT.md) §10.

Apple M3 Ultra, macOS 26.6.2, release `box` and `box-player`, script
projects (template player), medians of 5 exports:

| project | packed files | assets + project file | project folder | game.pak | .app | export | import (validation) | pak write | bundle | sign |
|---|---|---|---|---|---|---|---|---|---|---|
| collector_luau | 40 | 266 KB | 356 KB | 270 KB | 37.8 MB | 304 ms | 11 ms | 7 ms | 158 ms | 121 ms |
| cinder_keep | 48 | 653 KB | 793 KB | 657 KB | 38.2 MB | 237 ms | 11 ms | 8 ms | 120 ms | 83 ms |
| script_demo | 24 | 41 KB | 46 KB | 43 KB | 37.6 MB | 173 ms | 9 ms | 5 ms | 86 ms | 68 ms |

The pak is 1 % larger than the files it holds (header, index, alignment) and
24-28 % smaller than the project folder (editor state, authoring, docs
stay behind). The bundle is the player (36.6 MB release binary) plus the
pak; bundling time is mostly copying the binary and writing the icon set.

Native export (`examples/collector`, release player already built once):
11.2 s cargo (incremental), 0.5 s cook, bundle and sign; 16 files, 155 KB pak.

Startup (process start to exit after one headless frame, 640x360, medians
of 12 runs, the same release `box-player`):

| project | from the folder | from game.pak | the .app's executable |
|---|---|---|---|
| collector_luau | 150 ms | 149 ms | 169 ms |
| cinder_keep | 128 ms | 126 ms | 156 ms |
| script_demo | 125 ms | 106 ms | 98 ms |

Folder and pak are equal within noise: startup is dominated by GPU device
and pipeline creation, and reading the assets is under a millisecond either
way (spike S7: reading every file takes 0.07-0.1 ms from the pak against
0.4-0.6 ms as loose files). The `.app` column varies by ±30 ms between runs
(a freshly copied, signed binary in a new location).
