#!/usr/bin/env python3
"""Text of two saved copies of the track page, compared: what was removed and added.
Usage: pagediff.py OLD.html NEW.html"""
import difflib, html, re, sys


def text(path):
    s = open(path, encoding="utf-8", errors="replace").read()
    s = re.sub(r"(?s)<(script|style)[^>]*>.*?</\1>", " ", s)
    s = re.sub(r"(?i)<(br|/p|/div|/li|/h[1-6]|/tr|/td|/th)[^>]*>", "\n", s)
    s = html.unescape(re.sub(r"<[^>]+>", " ", s))
    return [re.sub(r"\s+", " ", l).strip() for l in s.split("\n") if l.strip()]


a, b = text(sys.argv[1]), text(sys.argv[2])
for line in difflib.unified_diff(a, b, "old", "new", lineterm="", n=0):
    print(line[:600])
