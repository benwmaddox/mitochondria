"""Pin a process to one logical CPU (Windows). This machine's CPU mixes performance and efficiency cores; a long-running
benchmark process can be moved to a slower core mid-run, which looks like a slowdown with use. BENCH_CPU picks the CPU
(default 10, measured fast: on this machine CPUs 0, 10 and 12-18 run a small loop in 80 ns, CPUs 2-8 in 121 ns, CPU 20 in 251 ns); BENCH_CPU=none leaves placement to Windows."""
import ctypes, os

def pin(proc):
    cpu = os.environ.get("BENCH_CPU", "10")
    if cpu == "none": return
    k = ctypes.windll.kernel32
    h = k.OpenProcess(0x1F0FFF, False, proc.pid)
    if h: k.SetProcessAffinityMask(h, 1 << int(cpu)); k.CloseHandle(h)
