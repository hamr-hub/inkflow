#!/usr/bin/env python3
"""inkflow · scripts/art-turn-check.py

Reject an autonomous art turn that only nudges existing numbers.

The loop spent twenty-odd consecutive turns multiplying one float constant by
1.025 and calling it an aesthetic decision. Each turn moved a pixel or two of a
shallow gradient, and the prose in the commit message grew to hide how little
had changed. The prompt already said "one small, high-value refinement" and
"never justify a change at length"; the loop ignored both. Prompts alone have
now been shown not to work, so the refusal is mechanical.

A turn is refused when every line it changed in src/ differs from HEAD only in
numeric literals, and each of those literals moved by less than NOISE. Anything
that adds, removes or reshapes a line passes — that is what a real change to the
piece looks like.

Usage:
    art-turn-check.py              # judge the working tree against HEAD
    art-turn-check.py HEAD~1..HEAD # judge a commit the loop just made
"""
import re
import subprocess
import sys

# A literal we are willing to call "the same number, nudged".
NOISE = 0.10

NUM = re.compile(r"(?<![A-Za-z0-9_.])(\d+\.\d+|\.\d+|\d+)(?![A-Za-z0-9_.])")


def changed_pairs(rev):
    """Classify the diff under src/ into rewrites and pure add/remove lines.

    Returns (pairs, structural) where `pairs` are (-old, +new) rewrites and
    `structural` is every added or removed line that had no counterpart. A pure
    addition is a structural change on its face — it cannot be a number nudge —
    so it must never be dropped, or the check refuses real work.
    """
    args = ["git", "diff", "-U0"]
    args += rev.split("..") if ".." in rev else [rev, "--"]
    args += ["--", "src/"]
    out = subprocess.run(
        args, capture_output=True, text=True, check=True,
    ).stdout

    pairs, structural, pending = [], [], []
    for line in out.splitlines():
        if line.startswith("---") or line.startswith("+++"):
            continue
        if line.startswith("@@"):
            # removals left over from the previous hunk had no addition
            structural += pending
            pending = []
        elif line.startswith("-"):
            pending.append(line[1:])
        elif line.startswith("+"):
            if pending:
                pairs.append((pending.pop(0), line[1:]))
            else:
                structural.append(line[1:])
    structural += pending  # trailing pure removals
    return pairs, structural


def shape(line):
    return NUM.sub("#", line)


def rel_delta(a, b):
    try:
        x, y = float(a), float(b)
    except ValueError:
        return 0.0
    if x == y:
        return 0.0
    return abs(y - x) / max(abs(x), 1e-9)


def main():
    rev = sys.argv[1] if len(sys.argv) > 1 else "HEAD"
    try:
        pairs = changed_pairs(rev)
    except subprocess.CalledProcessError as exc:
        print(f"art-turn-check: cannot diff {rev}: {exc}", file=sys.stderr)
        return 0  # never block the loop on our own tooling

    pairs, structural = pairs
    if not pairs and not structural:
        print("art-turn-check: turn changed nothing in src/ — nothing to judge")
        return 1

    if structural:
        print(f"art-turn-check: {len(structural)} line(s) added or removed outright "
              f"— turn accepted")
        return 0

    nudges, others = [], []
    for old, new in pairs:
        if shape(old) == shape(new):
            a = NUM.findall(old)
            b = NUM.findall(new)
            if a and len(a) == len(b):
                worst = max((rel_delta(x, y) for x, y in zip(a, b)), default=0.0)
                nudges.append((worst, old.strip(), new.strip()))
                continue
        others.append((old, new))

    if others:
        print(f"art-turn-check: {len(others)} line(s) are not a pure number nudge — turn accepted")
        return 0

    worst = max((n for n, _, _ in nudges), default=0.0)
    if worst >= NOISE:
        print(f"art-turn-check: numeric change of {worst:.1%} is above the {NOISE:.0%} "
              f"noise floor — turn accepted")
        return 0

    print(f"art-turn-check: REFUSING the turn. Every change is a number nudged by "
          f"at most {worst:.1%}, which is below what a viewer can see:")
    for _, old, new in nudges:
        print(f"    - {old}")
        print(f"    + {new}")
    print("Change something a viewer can see: the composition, the legibility, "
          "the depth, or the rhythm — or do not commit at all.")
    return 1


if __name__ == "__main__":
    sys.exit(main())
