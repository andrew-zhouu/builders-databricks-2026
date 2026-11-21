#!/usr/bin/env python3
"""Dump the visible text (and optionally script payloads) of an HTML file."""
import re
import sys
from html.parser import HTMLParser


class T(HTMLParser):
    def __init__(self):
        super().__init__()
        self.skip = 0
        self.out = []
        self.scripts = []
        self.in_script = False

    def handle_starttag(self, tag, attrs):
        if tag in ("script", "style"):
            self.skip += 1
            self.in_script = tag == "script"
        if tag in ("p", "div", "li", "h1", "h2", "h3", "h4", "tr", "br", "section"):
            self.out.append("\n")
        if tag == "a":
            href = dict(attrs).get("href")
            if href:
                self.out.append(f"[{href}] ")

    def handle_endtag(self, tag):
        if tag in ("script", "style"):
            self.skip -= 1
            self.in_script = False
        if tag in ("td", "th"):
            self.out.append(" | ")

    def handle_data(self, data):
        if self.skip:
            if self.in_script:
                self.scripts.append(data)
            return
        self.out.append(data)


p = T()
p.feed(open(sys.argv[1], encoding="utf-8", errors="replace").read())
text = "".join(p.out)
text = re.sub(r"\n\s*\n+", "\n", text)
print(text)
if len(sys.argv) > 2:
    print("\n\n===== SCRIPTS =====")
    for s in p.scripts:
        if len(s) > 200:
            print(s[:20000])
            print("-----")
