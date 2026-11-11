// Message batch filter, Go version (bounds-checked slices, append into a reused output slice). Same format, data and checksum as main.mito and rust/src/main.rs; see ../README.md.
package main

import (
	"fmt"
	"os"
	"runtime"
	"strconv"
	"time"
)

const maxPayload = 120

var seed uint64 = 42

func rnd(max uint64) uint64 {
	seed = (seed*3877 + 29573) % 139968
	return seed * max / 139968
}

func build() []byte {
	buf := make([]byte, 0, 32768)
	for i := uint64(0); i < 256; i++ {
		r := (i*37 + 11) % 256
		length := 16 + int(rnd(105))
		var cat byte
		magic, ver, flen := uint16(0x4D53), byte(1), length
		if r < 24 {
			cat = byte(rnd(8))
			switch r % 3 {
			case 0:
				magic = 0x4D54
			case 1:
				ver = 2
			default:
				flen = length + 1
			}
		} else if r < 88 {
			cat = 3 + byte(r&1)*2
		} else {
			cat = byte(rnd(6))
			if cat >= 3 {
				cat++
			}
			if cat >= 5 {
				cat++
			}
		}
		buf = append(buf, byte(magic), byte(magic>>8), ver, cat, byte(length+8), byte((length+8)>>8), byte(flen), byte(flen>>8))
		for j := 0; j < length; j++ {
			buf = append(buf, byte(rnd(256)))
		}
	}
	return buf
}

// one batch: filter and copy into out, then read the copy back; out is reset for the next batch
func process(input []byte, out []byte) (uint32, []byte, uint32, uint32, uint32) {
	var rejected, skipped, copied uint32
	off := 0
	for off < len(input) {
		magic := uint16(input[off]) | uint16(input[off+1])<<8
		ver := input[off+2]
		cat := input[off+3]
		frame := int(input[off+4]) | int(input[off+5])<<8
		length := int(input[off+6]) | int(input[off+7])<<8
		if magic != 0x4D53 || ver != 1 || length+8 != frame || length > maxPayload {
			rejected++
		} else if cat == 3 || cat == 5 {
			skipped++
		} else {
			out = append(out, input[off:off+frame]...)
			copied++
		}
		off += frame
	}
	var h uint32
	for i, b := range out {
		h += uint32(b ^ byte(i))
	}
	h += uint32(len(out)) << 16
	return h, out[:0], rejected, skipped, copied
}

func main() {
	iters := 500000
	if len(os.Args) > 1 {
		iters, _ = strconv.Atoi(os.Args[1])
	}
	if len(os.Args) > 2 && os.Args[2] == "allocs" {
		allocs(iters)
		return
	}
	if len(os.Args) > 2 && os.Args[2] == "cold" {
		cold(iters)
		return
	}
	warm := iters / 5
	if warm < 1 {
		warm = 1
	}
	input := build()
	out := make([]byte, 0, 32768)
	var result uint32
	var h, rej, skp, cpd uint32
	for i := 0; i < warm; i++ {
		h, out, rej, skp, cpd = process(input, out)
		result = result*31 + h
	}
	t := time.Now()
	for i := 0; i < iters; i++ {
		h, out, rej, skp, cpd = process(input, out)
		result = result*31 + h
	}
	fmt.Printf("%.6f %d %d %d %d\n", time.Since(t).Seconds(), result, rej, skp, cpd)
}

// cold start: no warmup; the time of each of the first batches, then the rest, and the heap allocations made during the batches
func cold(iters int) {
	input := build()
	out := make([]byte, 0, 32768)
	first := make([]int64, 0, 5)
	var ms0, ms1 runtime.MemStats
	runtime.ReadMemStats(&ms0)
	var result, h uint32
	for i := 0; i < 5 && i < iters; i++ {
		t1 := nowNs()
		h, out, _, _, _ = process(input, out)
		result = result*31 + h
		first = append(first, nowNs()-t1)
	}
	t := time.Now()
	for i := 5; i < iters; i++ {
		h, out, _, _, _ = process(input, out)
		result = result*31 + h
	}
	total := time.Since(t).Seconds()
	steady := iters - 5
	runtime.ReadMemStats(&ms1)
	fmt.Printf("first batches ns %v total %.6fs per batch (after the first 5) %.0f ns allocations during batches %d checksum %d\n", first, total, total*1e9/float64(steady), ms1.Mallocs-ms0.Mallocs, result)
}

// heap allocations only (no timers): during the first 5 batches and during the rest
func allocs(iters int) {
	input := build()
	out := make([]byte, 0, 32768)
	var ms0, ms1, ms2 runtime.MemStats
	var h uint32
	runtime.ReadMemStats(&ms0)
	for i := 0; i < 5; i++ {
		h, out, _, _, _ = process(input, out)
	}
	runtime.ReadMemStats(&ms1)
	for i := 5; i < iters; i++ {
		h, out, _, _, _ = process(input, out)
	}
	runtime.ReadMemStats(&ms2)
	fmt.Printf("allocations first5 %d steady %d (%d)\n", ms1.Mallocs-ms0.Mallocs, ms2.Mallocs-ms1.Mallocs, h)
}
