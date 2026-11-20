import io, sys
sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding="utf-8")
from markdownify import markdownify
html = '<h1>Title</h1><p>Hello <a href="https://example.com">world</a></p><ul><li>One</li><li>Two</li></ul>'
print(repr(markdownify(html)))
