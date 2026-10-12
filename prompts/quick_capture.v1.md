TASK: Turn the user's text into a task draft.

Current date/time: {{now}}
Timezone: {{timezone}}

<user_context_data>
User text: "{{capture_text}}"
</user_context_data>

Rules:
- Resolve relative dates ("Friday", "tomorrow night", "next week", "akhir bulan") to an
  exact ISO 8601 datetime with the timezone offset above, calculated from the current
  date/time. "Friday" means the next Friday from today, not this week's if today is
  already past it.
- If a date is given but no time, use 23:59 local as the deadline.
- If no date is mentioned at all, due_at is null. Do not invent a deadline.
- priority: 1 = high, 2 = normal, 3 = low. Use 1 only if the user signals urgency
  (words like urgent, ASAP, penting, besok ujian). Default to 2.
- Title: short, clear, and actionable. Move extra detail into notes.
- estimate_min: only set it if the user states or clearly implies a duration. Otherwise
  null.
- Do not invent details that are not in the text or implied by it.

Schema:
{"title":"string","notes":null,"due_at":null,"priority":2,"estimate_min":null}
