import { PageHeader, Section } from '@/components/layout/Section'
import { BrandIllustration } from '@/components/ui/BrandIllustration'
import { EmptyState } from '@/components/ui/States'

/**
 * The workspace list, and the first-run entry surface.
 *
 * This is where the brand illustration belongs: the design uses it on the entry
 * page, and an empty first-run screen is the app's equivalent moment. It is not
 * repeated on every screen, which is what keeps it a brand asset rather than a
 * sticker.
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
        subtitle="One workspace per course. Everything inside it reads across all of its sources."
      />

      <Section title="Your workspaces">
        <div className="card px-5 py-6">
          <div className="flex flex-col items-start gap-6 sm:flex-row sm:items-center">
            <BrandIllustration className="w-44 shrink-0 sm:w-52" />
            {/* `padded={false}`: the card supplies the padding, and the
                illustration sits beside the text, so a second set of padding
                would push the text off the illustration's left edge. */}
            <EmptyState
              padded={false}
              title="No workspaces yet"
              reason="Create one per course, then add its material: a PDF, slide deck, document, or link. Answers will cite the page they came from, and the sources stay searchable together rather than in separate notebooks."
            />
          </div>
        </div>
      </Section>
    </div>
  )
}
