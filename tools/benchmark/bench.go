// Head-to-head benchmark — Rust proxy vs Python proxy.
//
// Measures per-concurrency latency (avg/p50/p90/max), effective throughput,
// and CPU/memory of the proxy processes during each test.
//
// Build & run:
//   go build -o bench bench.go && ./bench
//
// Environment:
//   BENCH_UPSTREAM     — upstream URL         (default: http://127.0.0.1:9998/test.bin)
//   BENCH_RUST_URL     — Rust proxy endpoint  (default: http://127.0.0.1:8888)
//   BENCH_PYTHON_URL   — Python proxy endpoint(default: http://127.0.0.1:8889)
//   BENCH_API_PASSWORD — proxy api_password   (default: dedsec)
//   BENCH_FILE_MB      — size of upstream file in MB (default: 7)
//   BENCH_WARMUP       — global warmup rounds (default: 5)
//   BENCH_ROUNDS       — timed rounds per level (default: 5)
//   BENCH_CONCURRENCY  — comma-separated c levels (default: 10,20,30,50,100)
package main

import (
	"fmt"
	"io"
	"math"
	"net/http"
	"os"
	"os/exec"
	"runtime"
	"sort"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"
)

// ── Config ──────────────────────────────────────────────────────────────────

var (
	fileMB      = envInt("BENCH_FILE_MB", 7)
	warmupG     = envInt("BENCH_WARMUP", 5)
	rounds      = envInt("BENCH_ROUNDS", 5)
	timeout     = 120 * time.Second
	upstream    = envStr("BENCH_UPSTREAM", "http://127.0.0.1:9998/test.bin")
	apiPassword = envStr("BENCH_API_PASSWORD", "dedsec")
	rustBase    = envStr("BENCH_RUST_URL", "http://127.0.0.1:8888")
	pythonBase  = envStr("BENCH_PYTHON_URL", "http://127.0.0.1:8889")
)

var rustURL = fmt.Sprintf("%s/proxy/stream?d=%s&api_password=%s", rustBase, upstream, apiPassword)
var pyURL = fmt.Sprintf("%s/proxy/stream?d=%s&api_password=%s", pythonBase, upstream, apiPassword)

var client = &http.Client{
	Timeout: timeout,
	Transport: &http.Transport{
		MaxIdleConns:        500,
		MaxIdleConnsPerHost: 200,
		IdleConnTimeout:     90 * time.Second,
		DisableCompression:  true,
	},
}

// ── Proc stats (CPU% + RSS MB across all workers matching pattern) ───────────

type ProcStats struct{ CPU, MemMB float64 }

func procStats(pattern string) ProcStats {
	out, _ := exec.Command("bash", "-c",
		fmt.Sprintf("pgrep -f '%s' | while read p; do ps -p $p -o pcpu=,rss= 2>/dev/null; done", pattern)).Output()
	var cpu, mem float64
	for _, line := range strings.Split(strings.TrimSpace(string(out)), "\n") {
		f := strings.Fields(line)
		if len(f) >= 2 {
			c, _ := strconv.ParseFloat(f[0], 64)
			r, _ := strconv.ParseFloat(f[1], 64)
			cpu += c
			mem += r / 1024
		}
	}
	return ProcStats{CPU: cpu, MemMB: mem}
}

// ── Benchmark core ───────────────────────────────────────────────────────────

type BatchResult struct {
	Durs   []float64
	Errs   int
	WallMs float64
}

func runBatch(url string, c int) BatchResult {
	var mu sync.Mutex
	var wg sync.WaitGroup
	var durs []float64
	var ec int32
	ready := make(chan struct{})
	for i := 0; i < c; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			<-ready
			t0 := time.Now()
			resp, err := client.Get(url)
			if err != nil {
				atomic.AddInt32(&ec, 1)
				return
			}
			_, _ = io.Copy(io.Discard, resp.Body)
			_ = resp.Body.Close()
			if resp.StatusCode != 200 {
				atomic.AddInt32(&ec, 1)
				return
			}
			mu.Lock()
			durs = append(durs, float64(time.Since(t0).Milliseconds()))
			mu.Unlock()
		}()
	}
	t0 := time.Now()
	close(ready)
	wg.Wait()
	wall := float64(time.Since(t0).Milliseconds())
	sort.Float64s(durs)
	return BatchResult{durs, int(ec), wall}
}

type BenchResult struct {
	Avg, P50, P90, Max, TputMBs float64
	Ok, Err, C                  int
	CPU, MemMB                  float64
}

func bench(url string, c int, label, procPat string) *BenchResult {
	// local warmup
	for i := 0; i < 3; i++ {
		runBatch(url, c)
	}
	// timed rounds
	var batches []BatchResult
	var cpuS, memS []float64
	for i := 0; i < rounds; i++ {
		b := runBatch(url, c)
		batches = append(batches, b)
		if procPat != "" {
			s := procStats(procPat)
			cpuS = append(cpuS, s.CPU)
			memS = append(memS, s.MemMB)
		}
	}
	sort.Slice(batches, func(i, j int) bool { return batches[i].WallMs < batches[j].WallMs })
	m := batches[len(batches)/2]
	if len(m.Durs) == 0 {
		fmt.Printf("  %-18s c=%3d  FAILED (err=%d)\n", label, c, m.Errs)
		return nil
	}
	avg := 0.0
	for _, d := range m.Durs {
		avg += d
	}
	avg /= float64(len(m.Durs))
	tput := float64(len(m.Durs)) * float64(fileMB) / (m.WallMs / 1000)
	peakCPU, peakMem := 0.0, 0.0
	for _, v := range cpuS {
		peakCPU = math.Max(peakCPU, v)
	}
	for _, v := range memS {
		peakMem = math.Max(peakMem, v)
	}
	r := &BenchResult{
		Avg: avg, P50: m.Durs[len(m.Durs)/2],
		P90: m.Durs[int(float64(len(m.Durs))*0.9)],
		Max: m.Durs[len(m.Durs)-1],
		TputMBs: tput, Ok: len(m.Durs), Err: m.Errs, C: c,
		CPU: peakCPU, MemMB: peakMem,
	}
	fmt.Printf("  %-18s c=%3d  ok=%3d/%d  avg=%5.0fms  p50=%5.0fms  p90=%5.0fms  max=%5.0fms  %6.0fMB/s  cpu=%5.1f%%  mem=%5.0fMB  err=%d\n",
		label, c, r.Ok, c, r.Avg, r.P50, r.P90, r.Max, r.TputMBs, r.CPU, r.MemMB, r.Err)
	return r
}

// ── Main ─────────────────────────────────────────────────────────────────────

func main() {
	runtime.GOMAXPROCS(runtime.NumCPU())

	// Parse concurrency levels
	concStr := envStr("BENCH_CONCURRENCY", "10,20,30,50,100")
	var concs []int
	for _, s := range strings.Split(concStr, ",") {
		if n, err := strconv.Atoi(strings.TrimSpace(s)); err == nil {
			concs = append(concs, n)
		}
	}

	// Global warmup — hit all endpoints multiple times so cold starts don't
	// bias the first few measurements.
	fmt.Print("  Global warmup... ")
	for i := 0; i < warmupG; i++ {
		runBatch(upstream, 50)
		runBatch(rustURL, 50)
		runBatch(pyURL, 50)
	}
	fmt.Println("done")

	fmt.Println()
	fmt.Println(strings.Repeat("=", 120))
	fmt.Println("  MEDIAFLOW PROXY BENCHMARK — Rust vs Python")
	fmt.Printf("  Go %s  |  Upstream: %s  |  File: %d MB  |  %d global + 3 local warmup + %d rounds\n",
		runtime.Version(), upstream, fileMB, warmupG, rounds)
	fmt.Println(strings.Repeat("=", 120))

	type Row struct {
		C       int
		D, R, P *BenchResult
	}
	var rows []Row
	for _, c := range concs {
		fmt.Println()
		d := bench(upstream, c, "DIRECT", "")
		r := bench(rustURL, c, "RUST", "mediaflow-proxy-light")
		p := bench(pyURL, c, "PYTHON", "mediaflow-proxy.*python3")
		if r != nil && p != nil {
			rows = append(rows, Row{c, d, r, p})
		}
	}

	// Summary
	fmt.Println()
	fmt.Println(strings.Repeat("=", 120))
	fmt.Println("  SUMMARY")
	fmt.Println(strings.Repeat("=", 120))
	fmt.Printf("  %4s  %8s  %8s  %8s  %8s  %8s  %8s  %6s  %8s  %8s  %6s  %6s\n",
		"c", "Dir avg", "R avg", "P avg", "R vs P", "R tput", "P tput", "ratio",
		"R cpu", "P cpu", "R mem", "P mem")
	fmt.Println("  " + strings.Repeat("-", 114))
	for _, row := range rows {
		r, p := row.R, row.P
		dAvg := 0.0
		if row.D != nil {
			dAvg = row.D.Avg
		}
		spd := (p.Avg/r.Avg - 1) * 100
		ratio := r.TputMBs / p.TputMBs
		sign := "+"
		if spd < 0 {
			sign = ""
		}
		fmt.Printf("  %4d  %6.0fms  %6.0fms  %6.0fms  %s%4.0f%%  %6.0fMB/s  %6.0fMB/s  %5.2fx  %5.1f%%  %5.1f%%  %4.0fMB  %4.0fMB\n",
			row.C, dAvg, r.Avg, p.Avg, sign, spd, r.TputMBs, p.TputMBs, ratio,
			r.CPU, p.CPU, r.MemMB, p.MemMB)
	}
	fmt.Println()
}

// ── Helpers ──────────────────────────────────────────────────────────────────

func envStr(key, def string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return def
}

func envInt(key string, def int) int {
	if v := os.Getenv(key); v != "" {
		if n, err := strconv.Atoi(v); err == nil {
			return n
		}
	}
	return def
}
