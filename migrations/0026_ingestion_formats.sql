-- Phase 7, slice C: the uniform viewer the spec asks for.
--
-- PPTX and DOCX get a PDF preview rendered by LibreOffice (`soffice`), and the client
-- opens one media type for every source kind instead of learning three viewers. Null
-- when the converter is absent (A158: the source still ingests and the preview is simply
-- absent) or when the source has no original file at all (a `url` source stores no
-- bytes).
alter table sources add column preview_key text;
