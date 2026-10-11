export function Placeholder({ title, phase }: { title: string; phase: number }) {
  return (
    <section className="p-6">
      <h1 className="text-2xl font-semibold text-brand-900">{title}</h1>
      <p className="mt-2 text-sm text-gray-500">Implemented in Phase {phase}.</p>
    </section>
  )
}
