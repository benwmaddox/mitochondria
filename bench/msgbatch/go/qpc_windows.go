package main

import (
	"syscall"
	"unsafe"
)

// nowNs: the high-resolution performance counter (Go's own clock ticks about every 0.5 ms here, too coarse for one batch)
var (
	qpc  = syscall.NewLazyDLL("kernel32.dll").NewProc("QueryPerformanceCounter")
	qpf  = syscall.NewLazyDLL("kernel32.dll").NewProc("QueryPerformanceFrequency")
	freq int64
	cnt  int64 // global, so the call below allocates nothing
)

func nowNs() int64 {
	if freq == 0 {
		qpf.Call(uintptr(unsafe.Pointer(&freq)))
	}
	qpc.Call(uintptr(unsafe.Pointer(&cnt)))
	return cnt * 1000000000 / freq
}
