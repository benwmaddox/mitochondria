// Message batch filter, Goose version (NOT YET COMPILED: written from the Goose docs on 2026-11-10, no goose binary could be run then). Same format, data and
// checksum as ../main.mito, ../rust/src/main.rs and ../go/main.go; see ../README.md. Output is "<seconds> <checksum>", like the Rust and Go programs.

import std;
import os;

var seed: u64 = 42;

fn rnd(max: u64) -> u64 {
    seed = (seed * 3877 + 29573) % 139968;
    return seed * max / 139968;
}

fn build(buf: u8[>..]&) {
    for i in 256 {
        let r = (i as u64 * 37 + 11) % 256;
        let len = 16 + rnd(105);
        var cat: u64 = 0;
        var magic: u64 = 0x4D53;
        var ver: u64 = 1;
        var flen: u64 = len;
        if r < 24 {
            cat = rnd(8);
            if r % 3 == 0 { magic = 0x4D54; } else if r % 3 == 1 { ver = 2; } else { flen = len + 1; }
        } else if r < 88 {
            cat = 3 + (r & 1) * 2;
        } else {
            cat = rnd(6);
            if cat >= 3 { cat += 1; }
            if cat >= 5 { cat += 1; }
        }
        buf.push((magic & 255) as u8);
        buf.push((magic >> 8) as u8);
        buf.push(ver as u8);
        buf.push(cat as u8);
        buf.push(((len + 8) & 255) as u8);
        buf.push(((len + 8) >> 8) as u8);
        buf.push((flen & 255) as u8);
        buf.push((flen >> 8) as u8);
        for j in len { buf.push(rnd(256) as u8); }
    }
}

// one batch: filter and copy into out, then read the copy back; out is emptied for the next batch
fn process(input: const u8[:], out: u8[>..]&) -> u32 {
    var off: i64 = 0;
    while off < input.len {
        let magic = (input[off] as i64) | ((input[off + 1] as i64) << 8);
        let ver = input[off + 2];
        let cat = input[off + 3];
        let frame = (input[off + 4] as i64) | ((input[off + 5] as i64) << 8);
        let len = (input[off + 6] as i64) | ((input[off + 7] as i64) << 8);
        if magic != 0x4D53 || ver != 1 || len + 8 != frame || len > 120 {
            // rejected
        } else if cat == 3 || cat == 5 {
            // skipped
        } else {
            out.append(input[off..off + frame]);
        }
        off += frame;
    }
    var h: u32 = 0;
    for b, i in out { h = h + (b ^ (i & 255) as u8) as u32; }
    h = h + (out.len as u32 << 16);
    out.clear();
    return h;
}

fn main() {
    var iters: i64 = 500000;
    let a = args();
    if a.len > 1 {
        let n, ok = parse_int(a[1]);
        if ok { iters = n; }
    }
    let warm = max(iters / 5, 1);
    var input: u8[>..] = [];
    build(input);
    var out: u8[>..] = [];
    var result: u32 = 0;
    for i in warm { result = result * 31 + process(input, out); }
    let t0 = clock();
    for i in iters { result = result * 31 + process(input, out); }
    let t1 = clock();
    var line: u8[>..] = [];
    format_flt(line, t1 - t0, 6);
    print(line, " ", result);
}
