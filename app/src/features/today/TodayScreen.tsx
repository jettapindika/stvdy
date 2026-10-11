import { PageHeader, Section } from '@/components/layout/Section'
import { EmptyState } from '@/components/ui/States'

/**
 * Today, the default home.
 *
 * Every section is an honest empty state rather than a mock. There is no data
 * layer yet (that arrives in Phase 1), and a screen of invented study blocks and
 * due counts would be the worst kind of slop here: it would look like the
 * planner was working. Each state names what fills it instead.
 *
 * The four sections are the ones the planner and the study tools actually
 * produce, so the layout is real and will fill in as those phases land.
 */
export function TodayScreen() {
  return (
    <div className="mx-auto flex max-w-3xl flex-col gap-8 px-5 py-8">
      <PageHeader
        title="Today"
        subtitle="Study blocks, due cards, and deadlines, arranged by the time you have available."
      />

      <Section title="Study blocks">
        <div className="card">
          <EmptyState
            title="No study blocks scheduled"
            reason="Blocks appear once you set an exam date and add the material to cover. The planner works backward from the exam, fits sessions into your study hours, and never overlaps a class."
          />
        </div>
      </Section>

      <Section title="Due flashcards">
        <div className="card">
          <EmptyState
            title="No cards due"
            reason="Review cards appear here when their interval elapses. Generate a deck from a source and Stvdy schedules the reviews."
          />
        </div>
      </Section>

      <Section title="Tasks">
        <div className="card">
          <EmptyState
            title="Nothing due"
            reason="Tasks from a workspace, a syllabus import, or a quick capture land here, ordered by due date."
          />
        </div>
      </Section>

      <Section title="Upcoming exams">
        <div className="card">
          <EmptyState
            title="No exams tracked"
            reason="Add an exam as a goal and it becomes the anchor the planner schedules against."
          />
        </div>
      </Section>
    </div>
  )
}
