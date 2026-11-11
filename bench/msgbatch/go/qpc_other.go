//go:build !windows

package main

import "time"

func nowNs() int64 { return time.Now().UnixNano() }
