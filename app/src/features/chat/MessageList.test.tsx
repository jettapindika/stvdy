import { render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import { MessageList } from './MessageList'
import type { Message } from '@/api/queries/chat'

// MessageList needs i18n; the test render helper wires it, but we can also
// just mock the hook to return keys as-is.
vi.mock('react-i18next', () => ({
  useTranslation: () => ({
    t: (key: string) => {
      const map: Record<string, string> = {
        'chat.list.label': 'Chat',
        'chat.list.searching': 'Searching…',
        'chat.turn.you': 'You',
        'chat.turn.streaming': 'typing…',
        'chat.answer.disclaimer': 'AI-generated.',
        'chat.answer.copy': 'Copy',
        'chat.answer.saveToStudio': 'Save',
        'chat.answer.makeFlashcards': 'Flashcards',
        'chat.followUp.heading': 'Follow up',
        'chat.followUp.workedExample': 'Show example',
        'chat.followUp.flashcards': 'Make flashcards',
        'chat.empty.title': 'No questions yet',
        'honesty.noSourcesCited': 'No sources cited',
        'honesty.basedOnSources': '{{count}} sources',
        'honesty.basedOnSources_one': '1 source',
        'app.name': 'Stvdy',
        'chat.thinking.show': 'Show thinking',
        'chat.thinking.hide': 'Hide thinking',
        'chat.retry': 'Retry',
        // Callout labels
        'chat.callout.note': 'Note',
        'chat.callout.tip': 'Tip',
        'chat.callout.important': 'Important',
        'chat.callout.warning': 'Warning',
        'chat.callout.caution': 'Caution',
        'chat.callout.example': 'Example',
        'chat.callout.definition': 'Definition',
        'chat.callout.formula': 'Formula',
        'chat.callout.memorize': 'Memorize',
        'chat.callout.step': 'Step',
        'chat.callout.analogy': 'Analogy',
        'chat.callout.trap': 'Trap',
        'chat.callout.question': 'Question',
        'chat.callout.summary': 'Summary',
        'chat.callout.quote': 'Quote',
        'chat.callout.success': 'Success',
        'chat.callout.highlight': 'Highlight',
        'chat.callout.code': 'Code',
      }
      return map[key] ?? key
    },
  }),
}))

function msg(content: string): Message {
  return {
    id: 'm1',
    workspace_id: 'w1',
    role: 'assistant',
    content,
    citations: [],
    used_web: false,
    created_at: '2026-10-11T08:00:00Z',
  }
}

const noop = () => {}
const noTitle = () => ''

function renderAnswer(content: string) {
  return render(
    <MessageList
      messages={[msg(content)]}
      turn={null}
      sourceTitle={noTitle}
      onOpenCitation={noop}
      onRetry={noop}
      emptyReason=""
    />,
  )
}

// ---------------------------------------------------------------------------
// Block-level rendering
// ---------------------------------------------------------------------------

describe('AnswerText block rendering', () => {
  it('renders a fenced code block with language badge', () => {
    const { container } = renderAnswer('```python\nprint("hi")\n```')
    expect(container.querySelector('.answer-code-block')).not.toBeNull()
    expect(container.querySelector('.answer-code-lang')?.textContent).toBe('python')
    expect(container.querySelector('pre code')?.textContent).toBe('print("hi")')
  })

  it('renders a fenced code block without a language', () => {
    const { container } = renderAnswer('```\nsome code\n```')
    expect(container.querySelector('.answer-code-block')).not.toBeNull()
    expect(container.querySelector('.answer-code-lang')).toBeNull()
  })

  it('handles an unclosed fence without hanging', () => {
    const { container } = renderAnswer('```python\nline 1\nline 2')
    // Should not hang; code block still renders with available content
    expect(container.querySelector('.answer-code-block')).not.toBeNull()
  })

  it('renders headings h2–h4', () => {
    const { container } = renderAnswer('## Section\n\n### Subsection\n\n#### Detail')
    const headings = container.querySelectorAll('.answer-heading')
    expect(headings).toHaveLength(3)
    expect(headings[0]!.tagName).toBe('H2')
    expect(headings[1]!.tagName).toBe('H3')
    expect(headings[2]!.tagName).toBe('H4')
  })

  it('does not hang on a partial heading mid-stream (## with no text)', () => {
    const { container } = renderAnswer('Hello\n## \nWorld')
    // The safety guard advances past `## ` without looping
    expect(container.textContent).toContain('Hello')
    expect(container.textContent).toContain('World')
  })

  it('renders a horizontal rule', () => {
    const { container } = renderAnswer('Above\n\n---\n\nBelow')
    expect(container.querySelector('hr')).not.toBeNull()
  })

  it('renders a table with header and body', () => {
    const md = '| A | B |\n| --- | --- |\n| 1 | 2 |\n| 3 | 4 |'
    const { container } = renderAnswer(md)
    expect(container.querySelector('.answer-table')).not.toBeNull()
    const ths = container.querySelectorAll('th')
    expect(ths).toHaveLength(2)
    expect(ths[0]!.textContent).toBe('A')
    const tds = container.querySelectorAll('td')
    expect(tds).toHaveLength(4)
  })

  it('does not swallow a lone pipe line as a table', () => {
    const { container } = renderAnswer('The notation\n|x| means absolute value')
    // Both lines should appear as paragraph text, not be consumed by table detection
    expect(container.textContent).toContain('notation')
    expect(container.textContent).toContain('|x| means absolute value')
  })

  it('renders an ordered list', () => {
    const { container } = renderAnswer('1. First\n2. Second\n3. Third')
    const ol = container.querySelector('.answer-ol')
    expect(ol).not.toBeNull()
    expect(ol!.querySelectorAll('li')).toHaveLength(3)
  })

  it('renders an unordered list', () => {
    const { container } = renderAnswer('- Alpha\n- Beta')
    const ul = container.querySelector('.list-disc')
    expect(ul).not.toBeNull()
    expect(ul!.querySelectorAll('li')).toHaveLength(2)
  })

  it('renders a plain blockquote', () => {
    const { container } = renderAnswer('> Something wise')
    expect(container.querySelector('.answer-blockquote')).not.toBeNull()
  })
})

// ---------------------------------------------------------------------------
// Callout rendering
// ---------------------------------------------------------------------------

describe('callout blocks', () => {
  it('renders a [!TIP] callout with a custom title', () => {
    const { container } = renderAnswer('> [!TIP] Study smart\n> Use spaced repetition.')
    const callout = container.querySelector('.callout-tip')
    expect(callout).not.toBeNull()
    expect(callout!.querySelector('.answer-callout-title')?.textContent).toBe('Study smart')
    expect(callout!.querySelector('.answer-callout-body')?.textContent).toContain('spaced repetition')
  })

  it('renders a [!NOTE] callout with the i18n fallback label when no title is given', () => {
    const { container } = renderAnswer('> [!NOTE]\n> Extra context here.')
    const callout = container.querySelector('.callout-note')
    expect(callout).not.toBeNull()
    expect(callout!.querySelector('.answer-callout-title')?.textContent).toBe('Note')
  })

  it('resolves [!INFO] alias to note', () => {
    const { container } = renderAnswer('> [!INFO]\n> Aliased content.')
    expect(container.querySelector('.callout-note')).not.toBeNull()
  })

  it('resolves [!DANGER] alias to caution', () => {
    const { container } = renderAnswer('> [!DANGER]\n> Watch out.')
    expect(container.querySelector('.callout-caution')).not.toBeNull()
  })

  it('resolves [!HINT] alias to tip', () => {
    const { container } = renderAnswer('> [!HINT]\n> A helpful hint.')
    expect(container.querySelector('.callout-tip')).not.toBeNull()
  })

  it('falls through to a plain blockquote for an unknown [!TYPE]', () => {
    const { container } = renderAnswer('> [!BANANA]\n> Not a real callout.')
    // Should not render as a callout
    expect(container.querySelector('.answer-callout')).toBeNull()
    // Should render as a blockquote instead
    expect(container.querySelector('.answer-blockquote')).not.toBeNull()
  })

  it('renders each colour family', () => {
    const families: [string, string][] = [
      ['TIP', 'callout-tip'],
      ['IMPORTANT', 'callout-important'],
      ['CAUTION', 'callout-caution'],
      ['EXAMPLE', 'callout-example'],
      ['QUESTION', 'callout-question'],
    ]
    for (const [type, className] of families) {
      const { container } = renderAnswer(`> [!${type}]\n> Body.`)
      expect(container.querySelector(`.${className}`)).not.toBeNull()
    }
  })
})

// ---------------------------------------------------------------------------
// Inline rendering
// ---------------------------------------------------------------------------

describe('inline markup', () => {
  it('renders a link without leaking label or URL as text', () => {
    const { container } = renderAnswer('see [docs](https://example.com) here')
    const link = container.querySelector('a')
    expect(link).not.toBeNull()
    expect(link!.textContent).toBe('docs')
    expect(link!.getAttribute('href')).toBe('https://example.com')
    // The label "docs" and the URL must NOT appear as separate text nodes
    const text = container.textContent ?? ''
    // "docs" appears once (inside the link), not twice
    expect(text.split('docs').length - 1).toBe(1)
    // The raw URL must not appear as text
    expect(text).not.toContain('https://example.com')
  })

  it('rejects a javascript: URL (renders as plain text)', () => {
    const { container } = renderAnswer('[click](javascript:alert(1))')
    expect(container.querySelector('a')).toBeNull()
    expect(container.textContent).toContain('[click](javascript:alert(1))')
  })

  it('renders strikethrough', () => {
    const { container } = renderAnswer('this is ~~wrong~~ right')
    expect(container.querySelector('del')?.textContent).toBe('wrong')
  })

  it('renders bold and italic', () => {
    const { container } = renderAnswer('**bold** and *italic*')
    expect(container.querySelector('strong')?.textContent).toBe('bold')
    expect(container.querySelector('em')?.textContent).toBe('italic')
  })

  it('renders inline code', () => {
    const { container } = renderAnswer('use `Array.map()` here')
    expect(container.querySelector('code')?.textContent).toBe('Array.map()')
  })
})

// ---------------------------------------------------------------------------
// Combined: every block type in one answer
// ---------------------------------------------------------------------------

describe('full markdown answer', () => {
  it('renders every block type without hanging or losing content', () => {
    const md = [
      '## Overview',
      '',
      'The absolute value |x| is defined as:',
      '',
      '> [!DEFINITION] Absolute Value',
      '> |x| = x if x ≥ 0, |x| = -x if x < 0.',
      '',
      '```python',
      'def abs(x):',
      '    return x if x >= 0 else -x',
      '```',
      '',
      '| Input | Output |',
      '| --- | --- |',
      '| 5 | 5 |',
      '| -3 | 3 |',
      '',
      '1. Check the sign',
      '2. Negate if negative',
      '',
      '- Simple concept',
      '- Used everywhere',
      '',
      '> A wise quote.',
      '',
      '---',
      '',
      'See [Wikipedia](https://en.wikipedia.org/wiki/Absolute_value) for more.',
      '',
      '> [!INFO] Alias test',
      '> This uses the info→note alias.',
    ].join('\n')

    const { container } = renderAnswer(md)

    // Heading
    expect(container.querySelector('h2.answer-heading')?.textContent).toBe('Overview')
    // Paragraph with pipe (not swallowed)
    expect(container.textContent).toContain('|x| is defined')
    // Callout
    expect(container.querySelector('.callout-definition')).not.toBeNull()
    // Code block
    expect(container.querySelector('.answer-code-block')).not.toBeNull()
    expect(container.querySelector('.answer-code-lang')?.textContent).toBe('python')
    // Table
    expect(container.querySelectorAll('th')).toHaveLength(2)
    expect(container.querySelectorAll('td')).toHaveLength(4)
    // Ordered list
    expect(container.querySelector('.answer-ol')).not.toBeNull()
    // Unordered list
    expect(container.querySelector('.list-disc')).not.toBeNull()
    // Blockquote
    expect(container.querySelector('.answer-blockquote')).not.toBeNull()
    // HR
    expect(container.querySelector('hr')).not.toBeNull()
    // Link
    const link = container.querySelector('a')
    expect(link?.textContent).toBe('Wikipedia')
    expect(link?.getAttribute('href')).toBe('https://en.wikipedia.org/wiki/Absolute_value')
    // Alias callout (INFO → note)
    expect(container.querySelector('.callout-note')).not.toBeNull()
  })
})
