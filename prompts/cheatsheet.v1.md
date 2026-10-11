TASK: Build the content of a print-ready exam cheatsheet from the provided <sources> only.

Scope: {{scope}}   (e.g. "UTS Jaringan Komputer", chapter list, or a topic list)
Exam date: {{exam_date}}
Depth: {{depth}}   (brief | normal | thorough)

You are writing the CONTENT, not the layout. The application renders your JSON through a
fixed print template with its own CSS classes. Therefore:

- Never output HTML tags, CSS, or `<style>` anywhere. A field containing markup is a
  failure: it will be escaped and displayed literally.
- Choose the block "kind" that matches the content. Use the visual components where they
  genuinely help, not decoratively — a cheatsheet with a "Perumpamaan" box on every
  paragraph is worse than one with three good ones.
- Math goes in LaTeX inside "latex" fields only, using $...$ for inline and $$...$$ for
  display. Do not put LaTeX in "text" fields except short inline $...$ expressions.

Block kinds and when to use each:

- "text"      — a short explanatory paragraph. 1-4 sentences.
- "bullets"   — a list of distinct points. Use instead of a paragraph when items are
                parallel.
- "concept"   — a definition that must be memorized exactly. One term per block.
- "formula"   — a formula with its variable legend. "legend" explains each symbol and its
                unit; this is what makes a formula usable under exam pressure.
- "quickformula" — a shortcut, mnemonic rule, or approximation that trades accuracy for
                speed, plus when it is valid.
- "analogy"   — a real-world comparison that makes an abstract idea click. Use sparingly,
                at most one per section, and only where it actually clarifies.
- "trap"      — a mistake students actually make on this material: a confusion between two
                concepts, a sign error, an off-by-one, a case the theory doesn't cover.
- "memorize"  — the short list of keywords/values to have automatic. Keep it genuinely
                short; if everything is a keyword, nothing is.
- "table"     — comparisons, layer/state mappings, protocol fields. Set "wide": true when
                there are 5+ columns so the template can shrink the font.
- "question"  — an exam-style question with a worked answer. "likelihood" is your estimate
                of how likely this appears: high | medium | low. Answers must show the
                reasoning steps and end with the final answer.
- "diagram"   — an ASCII/text diagram drawn with characters inside a monospace block
                (boxes, arrows, layer stacks, sequence flows). Keep it under 60 characters
                wide so it never overflows the page.
- "svg"       — a simple vector diagram, only when a text diagram cannot express it.
                Allowed elements: svg, g, rect, circle, ellipse, line, polyline, polygon,
                path, text, tspan, defs, marker, title. No script, no style, no on*
                attributes, no href, no foreignObject, no external references. Use
                viewBox and no width/height so it scales to the page.

Content rules:
- Cover only what the exam covers, per the scope above. Do not include material that
  belongs to a later exam.
- Prioritise: what is most likely to be tested, what is most easily confused, and what is
  hardest to reconstruct from first principles under time pressure. Omit anything the
  student can trivially derive.
- Every factual claim must be traceable to the sources. Put the chunk labels in "labels"
  on the block that uses them, copied exactly as shown in <sources>. A block with no
  source support must be omitted, not guessed.
- Be honest about gaps: if the scope names a topic the sources do not cover, include the
  section with a single block of kind "text" that says so plainly.
- Structure: 2-6 chapters, each 2-5 sections, each section 2-8 blocks. A section should
  fit roughly on one printed page. Order chapters as the course teaches them.
- Density: this is a revision document, not a textbook. Prefer a table or bullets over
  prose. No filler, no "in this chapter we will learn" framing.
- Write all text in {{output_language}}.

Schema:
{"title":"string",
 "subtitle":"string",
 "course":null,
 "exam":null,
 "chapters":[
   {"title":"string",
    "sections":[
      {"title":"string",
       "blocks":[
         {"kind":"text","text":"string","labels":[1,2]},
         {"kind":"bullets","items":["string"],"labels":[3]},
         {"kind":"concept","term":"string","text":"string","labels":[1]},
         {"kind":"formula","latex":"$$...$$","caption":null,
          "legend":[{"symbol":"v","meaning":"string","unit":"m/s"}],"labels":[2]},
         {"kind":"quickformula","name":"string","latex":"$...$","note":"string","labels":[2]},
         {"kind":"analogy","text":"string","labels":[]},
         {"kind":"trap","text":"string","labels":[4]},
         {"kind":"memorize","items":["string"],"labels":[4]},
         {"kind":"table","caption":null,"wide":false,
          "headers":["string"],"rows":[["string"]],"labels":[5]},
         {"kind":"question","prompt":"string","likelihood":"high",
          "answer":{"steps":["string"],"final":"string"},"labels":[5]},
         {"kind":"diagram","caption":null,"text":"string","labels":[3]},
         {"kind":"svg","caption":null,"svg":"<svg viewBox=\"0 0 100 40\">...</svg>","labels":[3]}
       ]}
    ]}
 ]}
