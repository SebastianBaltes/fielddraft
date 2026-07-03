package main

import "testing"

// TestLeakScanClean — benign user-facing replies pass the filter.
func TestLeakScanClean(t *testing.T) {
	clean := []string{
		"Thanks for the report! We reproduced the crash and a fix is on the way.",
		"This doesn't quite fit the current vision, but here's an alternative you can try.",
		"Could you share which shape you were extruding when it happened?",
		"", // empty is trivially clean
		"Version 1.2 is live now — please refresh and let us know.",
		// A short git-sha-like hex run (<32 chars) must not trip the hex rule.
		"The fix landed in build deadbeef123, please update.",
		// A slash path NOT under a known repo root is fine.
		"You can find it under Settings/Preferences in the menu.",
		// Version-dotted tokens are not source-extension hits.
		"Fixed in v0.6.354 — thanks again!",
	}
	for _, txt := range clean {
		ok, reasons := leakScan(txt)
		if !ok {
			t.Errorf("expected clean, got reasons %v for %q", reasons, txt)
		}
	}
}

// TestLeakScanTrips — each leak class is detected.
func TestLeakScanTrips(t *testing.T) {
	cases := []struct {
		name string
		text string
	}{
		{"code fence", "Here's the fix:\n```go\nfunc main() {}\n```"},
		{"tilde fence", "~~~\nsome code\n~~~"},
		{"repo path server", "The bug is in server/tickets_handlers.go line 200."},
		{"repo path crates", "Look at crates/kernel/src/weave.rs for the cause."},
		{"repo path web", "It's in web/src/components/Viewer.tsx."},
		{"source extension", "Edit main.go and rebuild."},
		{"bearer token", "Use Authorization: Bearer abcdef0123456789abcdef."},
		{"password assignment", "Set PASSWORD=hunter2 in the env."},
		{"secret assignment", "api_key: sk-livesecretvalue"},
		{"long hex", "The session token is " + "deadbeef" + "deadbeef" + "deadbeef" + "deadbeef" + "."},
		{"private key", "-----BEGIN RSA PRIVATE KEY-----\nMIIE..."},
		{"sql migration path", "The schema change is in migrations/0002_tickets.sql."},
		{"yaml extension", "Adjust docker-compose.prod.yml and redeploy."},
		{"wgsl extension", "The raymarcher lives in raymarch.wgsl."},
		{"toml extension", "Bump the dependency in Cargo.toml."},
		{"docs path", "See docs/PARAMETRIC-WEAVING-PLAN.md for the plan."},
		{"long base64 run", "Decoded it as QWxhZGRpbjpvcGVuIHNlc2FtZUFsYWRkaW46b3BlbiBzZXNhbWU="},
		{"token assignment", "Set TOKEN=super-secret-value in the environment."},
		{"access key assignment", "The access_key: AKIAFOO is rotated."},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			ok, reasons := leakScan(c.text)
			if ok {
				t.Errorf("expected %q to trip the leak filter, but it passed", c.text)
			}
			if len(reasons) == 0 {
				t.Errorf("tripped but no reasons recorded for %q", c.text)
			}
		})
	}
}
