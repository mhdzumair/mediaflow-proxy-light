// Benchmark upstream server — serves a fixed N MB test file from memory.
//
// Build & run:
//   go build -o upstream upstream.go && ./upstream
//
// Environment:
//   BENCH_FILE   — path to test file (default: /tmp/test.bin, auto-generated)
//   BENCH_PORT   — port to listen on (default: 9998)
//   BENCH_SIZEMB — size of generated test file in MB if BENCH_FILE does not exist (default: 7)
package main

import (
	"crypto/rand"
	"fmt"
	"net/http"
	"os"
	"runtime"
	"strconv"
)

func main() {
	runtime.GOMAXPROCS(runtime.NumCPU())

	path := getEnv("BENCH_FILE", "/tmp/test.bin")
	port := getEnv("BENCH_PORT", "9998")
	sizeMB, _ := strconv.Atoi(getEnv("BENCH_SIZEMB", "7"))

	if _, err := os.Stat(path); os.IsNotExist(err) {
		fmt.Printf("Generating %d MB test file at %s...\n", sizeMB, path)
		buf := make([]byte, sizeMB*1024*1024)
		rand.Read(buf)
		if err := os.WriteFile(path, buf, 0644); err != nil {
			panic(err)
		}
	}

	data, err := os.ReadFile(path)
	if err != nil {
		panic(err)
	}
	clen := fmt.Sprintf("%d", len(data))

	http.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/octet-stream")
		w.Header().Set("Content-Length", clen)
		_, _ = w.Write(data)
	})

	fmt.Printf("Upstream: serving %d bytes on :%s (HTTP/1.1 keep-alive)\n", len(data), port)
	if err := http.ListenAndServe(":"+port, nil); err != nil {
		panic(err)
	}
}

func getEnv(key, def string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return def
}
