#!/usr/bin/env python3
"""Require exactly one documentation-impact choice in a pull request body."""

from __future__ import annotations

import os
import re
import sys


body = os.environ.get("PR_BODY", "")
updated = bool(re.search(r"- \[[xX]\] Documentation updated", body))
none = bool(re.search(r"- \[[xX]\] No documentation impact", body))

if updated == none:
    print(
        "Check exactly one PR documentation-impact option: "
        "'Documentation updated' or 'No documentation impact'.",
        file=sys.stderr,
    )
    raise SystemExit(1)

print("PR documentation impact is declared.")
