import { screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { Route, Routes } from 'react-router'

import { QuizScreen } from './QuizScreen'
import type { Quiz, QuizQuestion } from '@/api/queries/quizzes'
import { installFakePlatform, jsonResponse, mockFetch } from '@/test/harness'
import { renderWithProviders } from '@/test/render'
import { setAccessToken } from '@/lib/tokens'

/**
 * The two rules on this screen that protect the score rather than the layout.
 *
 * **An MCQ is graded by the server, a short answer is self-assessed.** The screen must
 * not offer a "was I right" control for an MCQ: the server decides it, and a student who
 * could disagree would be marking their own paper. The reverse matters too — a short
 * answer with no way to mark it would leave the question permanently ungraded.
 *
 * **The score is the server's.** The results panel must render the summary it was given
 * rather than recomputing one from the attempts the screen happens to hold, or a device
 * with a partial history would show a different score than the server reports. The
 * fixture below makes that observable: the server says 1 of 2 correct (50%) while the
 * screen holds one answer, which recomputed locally would be 100%.
 */

const QUIZ: Quiz = {
  id: 'q1',
  workspace_id: 'w1',
  title: 'Jaringan',
  difficulty: 'medium',
  question_types: 'mixed',
  source_ids: ['s1'],
  status: 'ready',
  error: null,
  requested_count: 2,
  question_count: 2,
  created_at: '2026-10-11T08:00:00Z',
}

const MCQ: QuizQuestion = {
  id: 'q-mcq',
  quiz_id: 'q1',
  kind: 'mcq',
  prompt: 'Apa fungsi three-way handshake?',
  options: [
    'Membuka koneksi TCP.',
    'Menutup koneksi TCP.',
    'Mengenkripsi data.',
    'Mengompres data.',
  ],
  answer: 'Membuka koneksi TCP.',
  explanation: 'Handshake menegosiasikan parameter sebelum data dikirim.',
  key_points: null,
  chunk_id: null,
  attempt: null,
}

const SHORT: QuizQuestion = {
  id: 'q-short',
  quiz_id: 'q1',
  kind: 'short',
  prompt: 'Mengapa UDP lebih cepat daripada TCP?',
  options: null,
  answer: 'Karena UDP tidak memakai handshake.',
  explanation: 'Tanpa handshake, tidak ada putaran tambahan sebelum data.',
  key_points: ['tidak ada handshake', 'tidak menjamin urutan'],
  chunk_id: null,
  attempt: null,
}

const RESULTS = {
  summary: {
    total: 2,
    answered: 2,
    graded: 2,
    correct: 1,
    score: 0.5,
    topics: [
      {
        source_id: 's1',
        source_title: 'Modul 3',
        page: 12,
        answered: 2,
        graded: 2,
        correct: 1,
      },
    ],
  },
  questions: [
    {
      question_id: 'q-mcq',
      kind: 'mcq',
      prompt: MCQ.prompt,
      answer: MCQ.answer,
      explanation: MCQ.explanation,
      chunk_id: null,
      user_answer: 'Membuka koneksi TCP.',
      correct: true,
      answered_at: '2026-10-11T09:00:00Z',
    },
    {
      question_id: 'q-short',
      kind: 'short',
      prompt: SHORT.prompt,
      answer: SHORT.answer,
      explanation: SHORT.explanation,
      chunk_id: null,
      user_answer: 'Karena tidak ada handshake.',
      correct: false,
      answered_at: '2026-10-11T09:01:00Z',
    },
  ],
}

beforeEach(() => {
  installFakePlatform('macos')
  setAccessToken('access-1')
})

afterEach(() => {
  vi.unstubAllGlobals()
})

/** A fetch that serves the quiz and records the attempts sent. */
function serve(
  attempts: { question_id: string; user_answer: string; correct?: boolean }[],
  // What the server already holds, as `GET /quizzes/:id` reports it. Empty for a fresh
  // quiz; populated to drive resumption.
  stored: Record<string, { id: string; user_answer: string; correct: boolean | null }> = {},
) {
  const withAttempt = (question: QuizQuestion): QuizQuestion => {
    // Bound once: indexing `stored` twice does not carry the narrowing from the first
    // read to the second, so the spread target stayed `… | undefined` and the fields
    // came back optional.
    const existing = stored[question.id]
    return {
      ...question,
      attempt:
        existing === undefined
          ? null
          : { answered_at: '2026-10-11T09:00:00Z', ...existing },
    }
  }
  mockFetch((url, init) => {
    const path = new URL(url).pathname
    if (path === '/quizzes/q1')
      return jsonResponse(200, { quiz: QUIZ, questions: [MCQ, SHORT].map(withAttempt) })
    if (path === '/quizzes/q1/results') return jsonResponse(200, RESULTS)
    if (path === '/workspaces/w1/sources') return jsonResponse(200, { sources: [] })
    if (path === '/quizzes/q1/attempts') {
      const body = JSON.parse(String(init?.body)) as {
        question_id: string
        user_answer: string
        correct?: boolean
      }
      attempts.push(body)
      // An MCQ's verdict comes from the server, so it is computed here from the stored
      // answer rather than read from the request: that is what the real route does.
      const correct =
        body.question_id === 'q-mcq'
          ? body.user_answer === MCQ.answer
          : (body.correct ?? null)
      return jsonResponse(200, {
        applied: true,
        attempt: { id: 'a1', question_id: body.question_id, correct },
      })
    }
    return jsonResponse(404, {
      code: 'not_found',
      message: 'no such route',
      min_plan: null,
      limit: null,
      used: null,
    })
  })
}

function renderQuiz() {
  return renderWithProviders(
    <Routes>
      <Route path="/quiz/:quizId" element={<QuizScreen />} />
    </Routes>,
    { route: '/quiz/q1' },
  )
}

describe('resuming a quiz that was already answered', () => {
  /**
   * A reload, or the second device the product is premised on, must not restart at
   * question 1 with the answers lost: the attempts are stored, so the screen opens
   * where the student left off, with the answered question revealed rather than
   * presented as still open. Without this the resumption code is unverified, which is
   * how the gap arrived in the first place.
   */
  it('opens on the first unanswered question, with the verdict already shown', async () => {
    const attempts: { question_id: string; user_answer: string; correct?: boolean }[] = []
    // The MCQ was answered and graded; the short answer was not touched.
    serve(attempts, {
      'q-mcq': { id: 'a-stored', user_answer: 'Menutup koneksi TCP.', correct: false },
    })
    renderQuiz()

    // It opens on the short answer, not on question 1.
    expect(await screen.findByText('Question 2 of 2')).toBeInTheDocument()
    expect(screen.getByText('Mengapa UDP lebih cepat daripada TCP?')).toBeInTheDocument()
    // And that question is still open, so it can be answered.
    expect(screen.getByRole('button', { name: 'Submit answer' })).toBeInTheDocument()
  })

  it('shows a stored answer as answered rather than open, and does not re-ask it', async () => {
    const attempts: { question_id: string; user_answer: string; correct?: boolean }[] = []
    // Both answered, so the quiz opens on its results.
    serve(attempts, {
      'q-mcq': { id: 'a1', user_answer: 'Membuka koneksi TCP.', correct: true },
      'q-short': { id: 'a2', user_answer: 'Karena tidak ada handshake.', correct: true },
    })
    renderQuiz()

    // The results view, from the server's summary, rather than a form asking again.
    expect(await screen.findByText('Your score')).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Submit answer' })).not.toBeInTheDocument()
    // Nothing was posted by merely opening the quiz: resumption reads, it does not write.
    expect(attempts).toHaveLength(0)
  })
})

describe('grading, per question kind', () => {
  it('offers no self-assessment for an mcq and one for a short answer', async () => {
    const attempts: { question_id: string; user_answer: string; correct?: boolean }[] = []
    serve(attempts)
    const user = userEvent.setup()
    renderQuiz()

    // The MCQ: choose the right option, submit, and the server's verdict appears with
    // no control asking the student to agree.
    await user.click(await screen.findByRole('radio', { name: /Membuka koneksi TCP\./ }))
    await user.click(screen.getByRole('button', { name: 'Submit answer' }))

    expect(await screen.findByText('Correct')).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'I got it' })).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'I missed it' })).not.toBeInTheDocument()

    // The request carried no `correct`: the client does not get to grade an MCQ.
    expect(attempts).toHaveLength(1)
    expect(attempts[0]?.correct).toBeUndefined()

    // The short answer: write it, submit, and the model answer and key points appear
    // with the two controls that mark it.
    await user.click(screen.getByRole('button', { name: 'Next question' }))
    await user.type(
      await screen.findByLabelText('Your answer'),
      'Karena tidak ada handshake.',
    )
    await user.click(screen.getByRole('button', { name: 'Submit answer' }))

    expect(await screen.findByText('Karena UDP tidak memakai handshake.')).toBeInTheDocument()
    expect(screen.getByText('tidak menjamin urutan')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'I got it' })).toBeInTheDocument()

    // Marking it sends the student's verdict, which is the one thing the client does
    // decide. The submit carried no verdict, so the mark is a second request.
    await user.click(screen.getByRole('button', { name: 'I missed it' }))
    expect(attempts).toHaveLength(3)
    expect(attempts[1]?.correct).toBeUndefined()
    expect(attempts[2]?.correct).toBe(false)
    expect(attempts[2]?.user_answer).toBe('Karena tidak ada handshake.')
  })

  it('renders the score the server reported rather than recomputing it', async () => {
    const attempts: { question_id: string; user_answer: string; correct?: boolean }[] = []
    serve(attempts)
    const user = userEvent.setup()
    renderQuiz()

    await user.click(await screen.findByRole('radio', { name: /Membuka koneksi TCP\./ }))
    await user.click(screen.getByRole('button', { name: 'Submit answer' }))
    await user.click(await screen.findByRole('button', { name: 'Next question' }))
    await user.type(await screen.findByLabelText('Your answer'), 'Karena tidak ada handshake.')
    await user.click(screen.getByRole('button', { name: 'Submit answer' }))
    await user.click(await screen.findByRole('button', { name: 'I missed it' }))

    // The server's summary says 1 of 2 (50%). Recomputing from the one answer this
    // screen sent would say 100%, so the number on screen is the server's.
    expect(await screen.findByText('50%')).toBeInTheDocument()
    expect(screen.queryByText('100%')).not.toBeInTheDocument()
    // "1 of 2 correct" appears twice by design: once beside the score and once in the
    // topic row, which is the same fact stated where the student acts on it.
    expect(screen.getAllByText('1 of 2 correct')).toHaveLength(2)

    // And the weak topic names its source and page, weakest first.
    expect(screen.getByText('Modul 3')).toBeInTheDocument()
    expect(screen.getByText('page 12')).toBeInTheDocument()
  })

  it('selects an mcq option with the number keys', async () => {
    const attempts: { question_id: string; user_answer: string; correct?: boolean }[] = []
    serve(attempts)
    const user = userEvent.setup()
    renderQuiz()

    // The key is pressed, not clicked: this is the only coverage of the number-key
    // contract, and it is the evidence gate 5's "reach every control by keyboard
    // alone" claim rests on. The handler listens on `window`, so it fires without
    // focusing a control first.
    await screen.findByText('Apa fungsi three-way handshake?')
    await user.keyboard('1')
    expect(screen.getByRole('radio', { name: /Membuka koneksi TCP\./ })).toBeChecked()

    // Submit by keyboard too, so the whole path is exercised without a pointer.
    await user.click(screen.getByRole('button', { name: 'Submit answer' }))
    expect(await screen.findByText('Correct')).toBeInTheDocument()

    // And once answered, a number key changes nothing: re-grading a question the
    // student already submitted would record a second judgement for one answer.
    await user.keyboard('2')
    expect(screen.getByRole('radio', { name: /Membuka koneksi TCP\./ })).toBeChecked()
  })
})
