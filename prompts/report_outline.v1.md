TASK: Propose an outline for a report on: {{topic}}.

Settings: audience/style {{style}}, length target {{length}}.

Rules:
- Base the outline on the provided <sources>. Each section lists the chunk labels that
  will support it, exactly as shown in <sources>.
- If an important part of the topic is not covered by the sources, still include the
  section (so the report has a sensible shape) but set "gaps": true. Be accurate about
  this — a section marked gaps: false must be fully supportable from the labels listed.
- 4-8 sections in logical order, typically introduction, body, conclusion. Each has a
  one-line purpose stating what the section will establish, not just its subject.
- Write in {{output_language}}.

Schema:
{"title":"string","sections":[{"heading":"string","purpose":"string","labels":[1,4],"gaps":false}]}
