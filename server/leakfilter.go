// Leak filter (§4.2) — defense-in-depth for OUTGOING public `po` comments.
//
// The structural guarantee is that fielddraft-po has no source access (§4.1),
// so it cannot leak code. The only residual channel is what fielddraft-coding
// reports to the PO internally; the PO might quote it into a public_reply. This
// filter is a conservative heuristic that flags such content. Anything it trips
// is stored with withheld=1 so users never see it, and is surfaced to the admin
// for review (§4.4 admin-awareness).
//
// It is deliberately a heuristic, not a parser: false positives (a withheld but
// benign comment) are acceptable — an admin can release it — whereas a missed
// leak is not. The rules:
//
//   1. Fenced code blocks — a line opening with ``` or ~~~ (markdown fences).
//   2. Repo file paths — tokens under a known source root (server/, crates/,
//      web/src/, docs/, …) or a path-like token ending in a source extension
//      (.go/.rs/.ts/.tsx/.js/.wgsl/.toml/.yml/.sql).
//   3. Secret / token patterns — long hex or base64 runs, a `Bearer ` token, a
//      `KEY=`/`PASSWORD=`/`SECRET=`/`TOKEN=` assignment, or a PEM private-key
//      header (`-----BEGIN … PRIVATE KEY-----`).

package main

import (
	"regexp"
	"strings"
)

// leakRepoRoots are directory prefixes that betray the private repo layout.
var leakRepoRoots = []string{
	"server/", "crates/", "web/src/", "web/", "docs/", "scripts/",
	"migrations/", "src/", "target/", "node_modules/",
}

// leakSourceExts are file extensions whose appearance in a path-like token
// signals a repo file reference.
var leakSourceExts = []string{
	".go", ".rs", ".ts", ".tsx", ".js", ".jsx", ".wgsl",
	".toml", ".yml", ".yaml", ".sql", ".mod", ".sum",
}

var (
	// A fenced code block opener at the start of a (trimmed) line.
	leakFenceRe = regexp.MustCompile("(?m)^[ \\t]*(```|~~~)")
	// A run of >=32 hex chars (e.g. a sha256, session token, git sha is short
	// so we require length to avoid flagging short hashes) — token-like.
	leakHexRe = regexp.MustCompile(`\b[0-9a-fA-F]{32,}\b`)
	// A long base64-ish run (>=40 of the base64 alphabet) — key/secret-like.
	leakBase64Re = regexp.MustCompile(`\b[A-Za-z0-9+/_-]{40,}={0,2}\b`)
	// `Bearer <token>` authorization material.
	leakBearerRe = regexp.MustCompile(`(?i)bearer\s+[A-Za-z0-9._-]{8,}`)
	// KEY=, PASSWORD=, SECRET=, TOKEN=, PASSWD= assignments with a value.
	leakAssignRe = regexp.MustCompile(`(?i)\b(password|passwd|secret|token|api[_-]?key|access[_-]?key|private[_-]?key)\s*[=:]\s*\S`)
	// A PEM private-key header.
	leakPEMRe = regexp.MustCompile(`-----BEGIN [A-Z ]*PRIVATE KEY-----`)
	// A path-like token: a/b/c with no spaces, used to test for repo paths.
	leakPathTokenRe = regexp.MustCompile(`[\w./-]*/[\w./-]+`)
	// A bare filename token (no slash needed) — caught when it ends in a known
	// source extension, e.g. "main.go".
	leakFileTokenRe = regexp.MustCompile(`[\w.-]+`)
)

// leakScan inspects an outgoing text and reports whether it is clean, plus the
// reasons it tripped (for admin review / logging). A clean text returns
// (true, nil); a tripped one returns (false, reasons).
func leakScan(text string) (clean bool, reasons []string) {
	if leakFenceRe.MatchString(text) {
		reasons = append(reasons, "fenced code block")
	}
	if leakPEMRe.MatchString(text) {
		reasons = append(reasons, "private-key header")
	}
	if leakBearerRe.MatchString(text) {
		reasons = append(reasons, "bearer token")
	}
	if leakAssignRe.MatchString(text) {
		reasons = append(reasons, "secret assignment")
	}
	if reason := leakScanPaths(text); reason != "" {
		reasons = append(reasons, reason)
	}
	// Token/secret runs last — broadest, most likely to overlap with the above.
	if leakHexRe.MatchString(text) {
		reasons = append(reasons, "long hex run")
	} else if leakBase64Re.MatchString(text) {
		// Only flag base64 if no hex already matched (hex ⊂ base64 alphabet);
		// avoids a duplicate reason for the same run.
		reasons = append(reasons, "long base64 run")
	}
	return len(reasons) == 0, reasons
}

// leakScanPaths flags repo file references: a path-like token under a known
// source root, or any token (path or bare filename) ending in a source
// extension. Returns the reason or "".
func leakScanPaths(text string) string {
	// Slash-bearing tokens under a known repo root.
	for _, tok := range leakPathTokenRe.FindAllString(text, -1) {
		t := strings.TrimSpace(tok)
		for _, root := range leakRepoRoots {
			if strings.HasPrefix(t, root) || strings.Contains(t, "/"+root) {
				return "repo path"
			}
		}
	}
	// Any token (path or bare filename) with a source-file extension. Trailing
	// dots are sentence punctuation, not part of the filename ("… in main.go.")
	// — strip them so a filename at sentence end still trips the rule.
	for _, tok := range leakFileTokenRe.FindAllString(text, -1) {
		t := strings.TrimRight(strings.TrimSpace(tok), ".")
		for _, ext := range leakSourceExts {
			if strings.HasSuffix(t, ext) && len(t) > len(ext) {
				return "repo path"
			}
		}
	}
	return ""
}
