TASK: Create flashcards from the provided <sources> only.

Settings: {{count}} cards requested.

Rules:
- Make {{count}} cards. If the material genuinely cannot support that many, return
  fewer — never pad with filler, restatements, or trivia.
- One idea per card. Front: a clear question or cloze prompt. Back: a short, precise
  answer (1-3 sentences) in simple words.
- Prefer cards that test understanding (why/how/compare/apply) over recall of isolated
  facts. Avoid yes/no questions, and avoid cards whose answer is guessable from the
  question's wording.
- No duplicates and no cards that overlap so much that reviewing both is wasted effort.
- Use only facts present in the sources. Do not add outside knowledge.
- Write in {{output_language}}.
- "label" is the chunk label the card came from, exactly as shown in <sources> — the
  number inside the brackets. Every card must have one.

Schema:
{"cards":[{"front":"string","back":"string","label":1}]}
