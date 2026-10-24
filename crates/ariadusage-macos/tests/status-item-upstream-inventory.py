#!/usr/bin/env python3
"""Map every targeted Swift @Test to a Rust marker, or explain the gap.

Run with python3 -I and --upstream pointing to the CodexBar baseline checkout.
This checks traceability, not assertion equivalence; review the mapped bodies too.
"""

import argparse
from collections import Counter
from pathlib import Path
import re
import subprocess
import sys

BASELINE = "b49d42eb53bbf79785b344492d594760dd163eac"
FILES = (
    "StatusItemAnimationTests.swift",
    "StatusItemAnimationSignatureTests.swift",
    "StatusItemIconObservationSignatureTests.swift",
    "StatusItemAnimationCapTests.swift",
    "StatusItemQuotaWarningFlashTests.swift",
    "StatusItemBlinkGatingTests.swift",
    "StatusItemAnimationCodexCreditsTests.swift",
)

# Each entry identifies the unavailable production contract, not a substitute scenario.
MISSING = {
    "StatusItemAnimationTests.swift": {
        285: "No OpenRouter key-limit projection or brand-versus-meter selection in the Rust frontend.",
        351: "No OpenRouter key-data fetch state or brand-versus-meter selection in the Rust frontend.",
        938: "No status-item menuBarDisplayText formatter for percent/pace display modes.",
        970: "No status-item combined-lane display-text formatter.",
        1011: "No status-item display-text formatter or pace-unavailable fallback.",
        1028: "No status-item display-text formatter or nil-pace fallback.",
        1047: "No status-item display-text path computing pace for the selected Claude metric.",
        1098: "No status-item combined session/weekly display-text formatter.",
        1150: "No status-item combined display-text formatter for weekly-only accounts.",
        1198: "No status-item combined display-text formatter with weekly pace in both mode.",
        1254: "No status-item formatter pairing session usage with weekly pace.",
        1312: "No status-item formatter pairing Codex session usage with weekly pace.",
        1369: "No status-item display-text formatter surfacing the exhausted weekly lane.",
        1426: "No status-item both-mode text formatter for a missing session lane.",
        1475: "No status-item combined display-text formatter for spend-limit-only accounts.",
        1534: "No status-item display-text formatter with Codex weekly-projection pace fallback.",
        1581: "No status-item display-text formatter for weekly-exhausted credit fallback.",
        1627: "No status-item display-text formatter for session-exhausted credit fallback.",
        1673: "No Kilo zero-total projection or status-item display-text formatter.",
        1719: "No brandImageWithStatusOverlay function; meter status overlays are a different contract.",
        1729: "No brandImageWithStatusOverlay function; meter status overlays are a different contract.",
    },
    "StatusItemAnimationSignatureTests.swift": {
        240: "No brand-percent button-title rendering/cache-hit repair path.",
        295: "No icon-only stale button-title repair path on a cached render.",
        343: "No inactive-display brand attachment or high-contrast content-mode setting.",
        428: "No merged status-icon deferral state while a menu tracks; menu-content deferral is distinct.",
        844: "Pure signature-cache assertions exist locally, but the required stale button-title/imagePosition repair path is absent.",
    },
    "StatusItemIconObservationSignatureTests.swift": {
        84: "No custom status-item layout tokens or layout-specific accessibility/tooltip policy.",
        118: "No custom account-label layout token in the icon observation contract.",
        141: "No hidden custom account-label layout token or its observation gate.",
        324: "No custom costToday/costWindow layout tokens in the icon observation contract.",
        340: "No custom costWindow layout dependency filtering in the icon observation contract.",
        359: "No direct tertiary-lane layout token in the icon observation contract.",
        380: "No direct lane layout-token dependency filtering in the icon observation contract.",
        402: "No custom OpenRouter balance layout token in the icon observation contract.",
        419: "No token-cost layout publication path driving status-icon refresh.",
        446: "No cached widget snapshot persistence in this Rust macOS frontend.",
        464: "No cached widget snapshot persistence in this Rust macOS frontend.",
        481: "No updateIcons precomputed store-signature parameter or observation-work memo.",
        495: "No updateIcons observation-work memo to compare with a recomputed signature.",
    },
    "StatusItemBlinkGatingTests.swift": {
        9: "No brand-icon mode, cancellable blink task/clock, or per-cycle icon performance counters.",
        42: "No brand-image injection/fallback path or cancellable blink-task clock.",
        73: "No stored status-item layout tokens or brand-image fallback path.",
        86: "No per-provider stored layout/static rendering or per-cycle icon render counters.",
    },
}


def main():
    if not sys.flags.isolated:
        raise SystemExit("Run this script with python3 -I.")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--upstream", type=Path, required=True)
    args = parser.parse_args()
    upstream = args.upstream.resolve()
    revision = subprocess.check_output(
        ["git", "-C", str(upstream), "rev-parse", "HEAD"], text=True
    ).strip()
    if revision != BASELINE:
        raise SystemExit("Upstream checkout is not the required baseline.")
    repo = Path(__file__).resolve().parents[3]
    declarations = {}
    for filename in FILES:
        source = (upstream / "Tests/CodexBarTests" / filename).read_text()
        tests = []
        for match in re.finditer(r"(?m)^\s{4}@Test[^\n]*\n\s+func `([^`]+)`", source):
            line = source.count("\n", 0, match.start()) + 1
            tests.append((filename, line, match[1]))
        if len(tests) != len(re.findall(r"(?m)^\s*@Test(?:\(|\s*$)", source)):
            raise SystemExit(f"Unparsed @Test declaration in {filename}")
        declarations[filename] = tests
    expected = {key for tests in declarations.values() for key in tests}
    mapped = {}
    for path in sorted((repo / "crates/ariadusage-macos/tests").rglob("*.rs")):
        source = path.read_text()
        pattern = r"(?m)^\s*// upstream: (\S+\.swift):(\d+) ([^\n]+)\n((?:\s*#\[[^\n]+\]\n)+)\s*fn (\w+)\("
        for match in re.finditer(pattern, source):
            filename, line, name, attributes, function = match.groups()
            if filename not in FILES:
                continue
            key = (filename, int(line), name)
            if key not in expected:
                raise SystemExit(f"Stale upstream marker: {filename}:{line} {name}")
            if "#[test]" not in attributes:
                raise SystemExit(f"Marker does not identify a Rust test: {function}")
            if key in mapped:
                raise SystemExit(f"Duplicate upstream marker: {filename}:{line} {name}")
            rust_line = source.count("\n", 0, source.index("fn " + function, match.start())) + 1
            mapped[key] = f"{path.relative_to(repo)}:{rust_line}::{function}"
    counts = Counter()
    print(f"Upstream baseline: {revision}")
    print("One row per @Test declaration; parameterized Swift arguments remain inside the mapped Rust test.")
    for filename in FILES:
        file_mapped = 0
        for key in declarations[filename]:
            _, line, name = key
            if key in mapped:
                print(f"{filename}:{line} {name} => {mapped[key]}")
                file_mapped += 1
            else:
                reason = MISSING.get(filename, {}).get(line)
                if reason is None:
                    raise SystemExit(f"Unexplained MISSING: {filename}:{line} {name}")
                print(f"{filename}:{line} {name} => MISSING: {reason}")
        total = len(declarations[filename])
        print(f"SUMMARY {filename}: {file_mapped}/{total} mapped, {total - file_mapped} MISSING")
        counts["total"] += total
        counts["mapped"] += file_mapped
    print(f"TOTAL: {counts['mapped']}/{counts['total']} mapped, {counts['total'] - counts['mapped']} MISSING")


if __name__ == "__main__":
    main()
