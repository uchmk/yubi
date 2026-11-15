# hook-probe: WH_MOUSE_LL 実験の手順（M3-0 ゲート）

LL マウスフックで「トラックパッドに触れている間のカーソル移動・クリックを止められるか」と、Raw Input とフックの到着順を、実機で確かめる開発用の道具。

- 場所: `src/bin/hook-probe.rs`（egui なし。`src/input` にも依存しない）
- 起動: `cargo run --bin hook-probe -- [--suppress] [--seconds N]`（秒数の既定は 20）
- ビルドは R:（RAM ディスク）。事前に `pwsh -NoProfile -File %LOCALAPPDATA%\uchmk-build\cargo-ramdisk.ps1` を回しておく。
- 管理者権限は要らない（管理者のウィンドウの上にカーソルがあるときは、フックにイベントが来ないことがある）。

## 準備

1. yubi 本体を閉じる（同じトラックパッドを Raw Input で読むので、混ざらないように）。
2. トラックパッドを USB で接続する（公式ドライバー v2.3）。USB のマウスも 1 つ繋ぐ。
3. 起動すると、先頭に `# PTP touch pad: ...`（トラックパッド）と `# mouse: ... trackpad_sibling=...`（マウス一覧）が出る。
   - `trackpad_sibling=true` のマウスがあれば、それがトラックパッドと同じ VID/PID の「兄弟マウス」。
   - 一覧に兄弟マウスが無い場合は、カーソルの移動が Raw Input のマウスとしては来ない（Windows の精密タッチパッド経路）可能性がある。それも結果のうち。

## 実験 1: 観察モード（何も止めない）

```
cargo run --bin hook-probe -- --seconds 20 > observe.txt
```

20 秒の間に次を順にやる（1 つ 3〜4 秒）:

1. トラックパッドで指 1 本でカーソルを動かす。
2. 指 2 本でスクロールする。
3. トラックパッドを押し込んでクリックする。
4. USB マウスを動かす・クリックする。

出力の見方:

| 行 | 意味 |
|---|---|
| `RI-PTP contacts=N` | Raw Input のトラックパッド（指の本数）|
| `RI-MOUSE dev=.. dx dy buttons` | Raw Input のマウス（`(trackpad)` は兄弟マウス）|
| `LL move x y flags injected=? extra=0x..` | LL フックが見た移動（`injected=true` は SendInput などの注入）|
| `LL lbutton down/up` | LL フックが見たボタン |

行頭の数字は開始からの経過 µs（QPC）。最後の summary に、件数・RI→LL の遅延の中央値・相関率が出る。

## 実験 2: 抑止モード

```
cargo run --bin hook-probe -- --suppress --seconds 20 > suppress.txt
```

指が 1 本以上トラックパッドに触れている間、フックが「注入ではない」移動とボタンを捨てる（捨てた行に `SWALLOWED`）。
Down を捨てたボタンの Up は、指が離れていても捨てる（ボタンが押しっぱなしにならないように）。

20 秒の間に次を順にやる:

1. トラックパッドに指を置いて動かす。カーソルが止まるか目で見る。置いて、離して、を 10 回ほど繰り返す（最初の移動が漏れるかを見る）。
2. 指を置いたまま押し込む。クリックが入らないか見る。
3. 指を置いたまま USB マウスを動かす。USB マウスのカーソルも止まるかを目で見る。
4. 指を離して、USB マウスとトラックパッドが普通に動くことを確かめる（止まったままなら、`Ctrl+C` かウィンドウを閉じる）。

## summary の読み方（記録して持ち帰る項目）

- `Raw Input: ... mouse(trackpad)=` が 0 なら、トラックパッドの移動は兄弟マウスとして来ていない。
- `median RI->LL latency`: 負なら Raw Input が先、正ならフックが先（スレッドが処理した順）。
- `correlation ... trackpad RI-MOUSE within 2 ms`: LL の移動のうち、トラックパッドの Raw Input マウスと 2 ms 以内で対応が付いた割合。`RI first` はフックの時点で相関が取れる（使える）数、`hook first` はフックの後に Raw Input が来た数。
- 抑止モード:
  - `first move after touch-down: leaked=N`: 触れた直後の最初の移動が漏れた回数（競合の証拠）。`moves that passed the hook within 30 ms before a touch-down` も参考。
  - `other-mouse RI-MOUSE events while touching` と実験 2-3 の目視: USB マウスの移動も止まった（巻き込まれた）なら、フックは元の機器を区別できない。
  - `swallowed: moves / buttons`: 捨てた数。

## 結果の使い方

- 最初の移動が漏れない（leaked=0）かつ USB マウスが巻き込まれない → カーソル固定・クリック差し替えを LL フックで進められる。
- 漏れる、または巻き込まれる → カーソル固定・クリック差し替えは縮小するか fork ドライバー案に戻す（設計 §6 M3-0）。

結果の `observe.txt` と `suppress.txt` の summary を、目視の結果（止まった・漏れた・巻き込まれた）と一緒に報告する。
