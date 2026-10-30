#!/usr/bin/env python3
"""Check buildless Pages assets and local navigation without network access."""
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import unquote, urlsplit
import re


class Page(HTMLParser):
    def __init__(self):
        super().__init__()
        self.ids = set()
        self.links = []
        self.h1s = 0
        self.lang = None
        self.viewport = None

    def handle_starttag(self, tag, attributes):
        attrs = dict(attributes)
        if "id" in attrs:
            assert attrs["id"] not in self.ids, f"Duplicate id: {attrs['id']}"
            self.ids.add(attrs["id"])
        if tag == "html":
            self.lang = attrs.get("lang")
        if tag == "h1":
            self.h1s += 1
        if tag == "meta" and attrs.get("name") == "viewport":
            self.viewport = attrs.get("content", "")
        if tag == "img":
            assert "alt" in attrs and "width" in attrs and "height" in attrs
        if tag in {"a", "link"} and attrs.get("href"):
            self.links.append(attrs["href"])
        if tag in {"script", "img"} and attrs.get("src"):
            assert not urlsplit(attrs["src"]).scheme, "Use local scripts and images"
            self.links.append(attrs["src"])


def main():
    root = Path(__file__).resolve().parents[1] / "site"
    pages = {}
    for path in sorted(root.rglob("*.html")):
        page = Page()
        page.feed(path.read_text(encoding="utf-8"))
        assert page.h1s == 1 and page.lang in {"ja", "en"}, path
        assert "width=device-width" in (page.viewport or ""), path
        assert "user-scalable=no" not in page.viewport, path
        pages[path.resolve()] = page
    assert {path.relative_to(root.resolve()).as_posix() for path in pages} == {"index.html", "en/index.html"}
    for path, page in pages.items():
        for value in page.links:
            link = urlsplit(value)
            assert link.scheme in {"", "https"}, value
            if link.scheme:
                continue
            target = (path.parent / unquote(link.path)).resolve() if link.path else path
            assert target.is_relative_to(root.resolve()), value
            if target.is_dir():
                target /= "index.html"
            assert target.is_file(), f"Missing local target: {value} in {path.name}"
            if link.fragment:
                assert target in pages and unquote(link.fragment) in pages[target].ids, value
    css = (root / "styles.css").read_text(encoding="utf-8")
    assert not re.search(r"@import|url\(\s*['\"]?https?://", css, flags=re.I)
    assert "prefers-reduced-motion" in css and ":focus-visible" in css
    assert (root / ".nojekyll").is_file()
    print("Verified Japanese/English HTML, local links/assets, viewport, focus and reduced motion")


if __name__ == "__main__":
    main()
