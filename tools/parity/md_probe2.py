import io, sys
sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding="utf-8")
from markdownify import markdownify
cases = {
 "h1": "<h1>One</h1>", "h2": "<h2>Two</h2>", "h3": "<h3>Three</h3>", "h6": "<h6>Six</h6>",
 "p": "<p>A</p><p>B</p>",
 "link": '<p>Hello <a href="https://x.com">world</a>.</p>',
 "strong": "<p>a <strong>b</strong> c</p>",
 "em": "<p>a <em>b</em> c</p>",
 "code": "<p>use <code>x=1</code> now</p>",
 "ul": "<ul><li>One</li><li>Two</li></ul>",
 "ol": "<ol><li>One</li><li>Two</li></ol>",
 "nested": "<ul><li>A<ul><li>B</li></ul></li></ul>",
 "pre": "<pre>line1\nline2</pre>",
 "br": "<p>a<br>b</p>",
 "entities": "<p>a &amp; b &lt;c&gt; &#65;</p>",
 "script": "<script>x=1</script><p>Body</p>",
}
for name, html in cases.items():
    print(f"{name}: {markdownify(html)!r}")
