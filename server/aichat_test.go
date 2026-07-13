package main

import (
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"
)

// The chat proxy spends real money, so its gate is worth pinning (ticket #347).

func TestAIChatKeyFromFileBeatsEnv(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "openrouter.key")
	if err := os.WriteFile(path, []byte("  sk-or-from-file\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	t.Setenv("OPENROUTER_API_KEY", "sk-or-from-env")
	t.Setenv("OPENROUTER_API_KEY_FILE", path)

	// A FILE is the deployment path: an env var leaks into `docker inspect` and
	// every child process; the file does not. So it wins when both are set.
	if got := aichatAPIKey(); got != "sk-or-from-file" {
		t.Fatalf("aichatAPIKey() = %q, want the file's content (trimmed)", got)
	}

	// An unreadable file must NOT silently fall back to the env var — that would
	// turn a misconfigured secret mount into "runs on the wrong key".
	t.Setenv("OPENROUTER_API_KEY_FILE", filepath.Join(dir, "missing"))
	if got := aichatAPIKey(); got != "" {
		t.Fatalf("unreadable key file: got %q, want \"\" (feature off)", got)
	}
}

func TestAIChatDailyQuota(t *testing.T) {
	q := newDailyQuota(2)
	for i := 0; i < 2; i++ {
		if !q.allow("u1") {
			t.Fatalf("request %d rejected inside the budget", i+1)
		}
	}
	if q.allow("u1") {
		t.Fatal("third request passed — the daily cap did not hold")
	}
	// The cap is PER USER: one user burning their budget must not lock out others.
	if !q.allow("u2") {
		t.Fatal("a different user was locked out by someone else's usage")
	}
	// 0 = disabled.
	if !newDailyQuota(0).allow("u1") {
		t.Fatal("quota 0 must disable the cap")
	}
}

func TestAIChatCrossSiteBlocked(t *testing.T) {
	post := func(withHeader bool, origin string) *http.Request {
		r := httptest.NewRequest(http.MethodPost, "http://internal:8080/aichat/x", nil)
		if withHeader {
			r.Header.Set("X-Target-Path", "chat/completions")
		}
		if origin != "" {
			r.Header.Set("Origin", origin)
		}
		return r
	}

	// The app's own request: custom header present. Note the Host is the INTERNAL
	// one (a proxy dialled it) while the Origin is the PUBLIC one — comparing the
	// two would reject this, which is exactly the bug this replaced.
	if crossSiteBlocked(post(true, "https://cad.example")) {
		t.Fatal("the app's own POST was blocked")
	}

	// A classic CSRF form POST cannot set headers → blocked.
	if !crossSiteBlocked(post(false, "https://evil.example")) {
		t.Fatal("a header-less cross-site POST passed")
	}

	// GET is metadata, spends nothing, and carries no model.
	get := httptest.NewRequest(http.MethodGet, "http://internal:8080/aichat/models", nil)
	if crossSiteBlocked(get) {
		t.Fatal("GET was blocked")
	}

	// With an explicit origin allowlist, a foreign Origin is rejected even if it
	// somehow carried the header.
	t.Setenv("AICHAT_ALLOWED_ORIGINS", "https://cad.example")
	if crossSiteBlocked(post(true, "https://cad.example")) {
		t.Fatal("the allowlisted origin was rejected")
	}
	if !crossSiteBlocked(post(true, "https://evil.example")) {
		t.Fatal("a foreign origin passed the allowlist")
	}
}
