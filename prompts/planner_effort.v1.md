TASK: Estimate study effort for an exam goal.

Target date: {{target_date}}

<user_context_data>
goal_title: "{{goal_title}}"
</user_context_data>

Topics (each with its source size and current quiz accuracy): {{topics_json}}

Rules:
- Return estimates only. Do not schedule anything, do not propose times or dates, and do
  not comment on the plan. The scheduler is deterministic code that runs after you.
- For each topic, estimate the total minutes needed to prepare well: a first pass
  through the material, practice, and one review. Estimate preparation, not reading time
  alone.
- Weight by evidence: lower quiz accuracy means more time. Larger source size means more
  time. A topic with high accuracy and little material needs only review.
- Use only the topics given. Do not invent topics, merge them, or drop them. Return
  exactly one entry per input topic, using the same topic names.
- "reason" is one short sentence naming the factors that drove the number.
- total_estimate_min must equal the sum of the topic estimates.
- Write in {{output_language}}.

Schema:
{"topics":[{"topic":"string","estimate_min":120,"reason":"string"}],"total_estimate_min":120}
