import { PageHeader, Section } from '@/components/layout/Section'
import { EmptyState } from '@/components/ui/States'

/**
 * The workspace list.
 *
 * A workspace is one course or subject, and the point of it is that many sources
 * live together so retrieval can cross them (the product's first differentiator).
 * The empty state says that, because "no workspaces" alone would not tell a new
 * student what a workspace is for.
 */
export function WorkspaceListScreen() {
  return (
    <div className="mx-auto flex max-w-3xl flex-col gap-8 px-5 py-8">
      <PageHeader
        title="Workspaces"
        subtitle="One workspace per course. Everything inside it, chat, flashcards, quizzes, and reports, reads across all of its sources."
      />

      <Section title="Your workspaces">
        <div className="panel">
          <EmptyState
            title="No workspaces yet"
            reason="Create one per course, then add its material: a PDF, slide deck, document, or link. Answers will cite the page they came from, and the sources stay searchable together rather than in separate notebooks."
          />
        </div>
      </Section>
    </div>
  )
}
