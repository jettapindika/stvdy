TASK: Break this task into concrete subtasks.

<user_context_data>
Task: "{{task_title}}"
Due: {{due_at}}
Notes: {{notes}}
</user_context_data>

Rules:
- 3-8 subtasks. Each title starts with a verb and describes a unit of work small enough
  to finish in one sitting (15-90 minutes).
- estimate_min is a realistic single-session estimate in minutes, not an optimistic one.
  Use a value in the 15-90 range.
- Order them logically: what must happen first, and what depends on what.
- Do not pad the list to reach a count, and do not add subtasks that restate the parent
  task.
- total_estimate_min must equal the sum of the subtask estimates.
- Write in {{output_language}}.

Schema:
{"subtasks":[{"title":"string","estimate_min":45}],"total_estimate_min":180}
