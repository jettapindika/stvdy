TASK: Create a quiz from the provided <sources> only.

Settings: {{count}} questions, difficulty {{difficulty}} (easy | medium | hard), types
{{types}} (mcq | short | mixed).

Rules:
- Test understanding and application, not recall of trivial details. Match the stated
  difficulty: easy = direct definitions and single-step recall; medium = apply a
  concept or compare two; hard = multi-step reasoning, edge cases, or combining topics.
- MCQ: exactly 4 options, exactly one correct. Distractors must be plausible and
  reflect common misconceptions, not obvious throwaways. Never use "all of the above",
  "none of the above", or "both A and B". Vary which position holds the correct answer
  across questions; do not let a pattern emerge.
- Short answer: "options" is null. "answer" is a concise model answer, and
  "key_points" lists the specific ideas a grader should look for.
- Every question has a 1-3 sentence explanation of why the answer is correct, grounded
  in the sources, and the chunk label it came from.
- For MCQ, "answer" is the exact text of the correct option. Copied verbatim from
  "options", not a letter. Do not include the letter prefix in either field.
- Use only facts present in the sources. If the sources cannot support {{count}}
  questions, return fewer rather than inventing material.
- Write in {{output_language}}.

Schema:
{"questions":[
  {"kind":"mcq","prompt":"string","options":["string","string","string","string"],
   "answer":"string","explanation":"string","label":2,"key_points":null},
  {"kind":"short","prompt":"string","options":null,
   "answer":"string","explanation":"string","label":3,"key_points":["string"]}
]}
