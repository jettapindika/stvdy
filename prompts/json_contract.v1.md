OUTPUT CONTRACT (applies to this whole task):

- Reply with a single JSON object and nothing else. No markdown fences, no leading or
  trailing commentary, no explanation before or after.
- Use exactly the field names and nesting shown in the schema. Do not add fields.
- Use null for a value that is unknown or not stated. Never invent a value to fill a
  field, and never write placeholder text like "N/A" or "..." into a field. Use null.
- Strings must be valid JSON strings: escape internal double quotes and newlines. Do
  not use trailing commas.
- If you genuinely cannot produce a valid object, return the schema with empty arrays
  and nulls rather than prose.
- Write all human-readable text in {{output_language}}.
