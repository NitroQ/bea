import { useState } from 'react';
import type { ClarificationQuestion } from './chatActions';
import Icon from './Icon';

export type ClarificationAnswer = { question: string; answer: string };
type Props = {
  questions: ClarificationQuestion[];
  busy: boolean;
  /// Called with every answered/annotated question when the user submits.
  onAnswered: (answers: ClarificationAnswer[]) => void;
  /// Called when the user closes the wizard — the caller stops generation.
  onClose: () => void;
};

/// One question at a time (the old all-at-once list produced double
/// scrollbars and an unreachable submit button on long transcripts). Each
/// question can be skipped with Next>, expanded with a free-text "details"
/// note for Bea, and the header Close stops minutes generation entirely.
export default function ClarificationsWizard({ questions, busy, onAnswered, onClose }: Props) {
  const [index, setIndex] = useState(0);
  const [answers, setAnswers] = useState<Record<number, string>>({});
  const [details, setDetails] = useState<Record<number, string>>({});
  const [showDetails, setShowDetails] = useState<Record<number, boolean>>({});
  const total = questions.length;
  const current = questions[index];
  const isLast = index === total - 1;
  const answeredCount = Object.values(answers).filter((value) => value.trim()).length;

  if (!current) return null;

  const setAnswer = (value: string) => setAnswers((values) => ({ ...values, [index]: value }));
  const next = () => setIndex((value) => Math.min(total - 1, value + 1));
  const back = () => setIndex((value) => Math.max(0, value - 1));

  const submit = () => {
    const pairs = questions.map((item, position) => {
      const answer = (answers[position] ?? '').trim();
      const extra = (details[position] ?? '').trim();
      if (answer && extra) return { question: item.question, answer: `${answer}. Additional context: ${extra}` };
      if (answer) return { question: item.question, answer };
      if (extra) return { question: item.question, answer: `No single answer — user context: ${extra}` };
      return { question: item.question, answer: '' };
    }).filter((pair) => pair.answer.trim());
    onAnswered(pairs);
  };

  return <div className="clarify-wizard">
    <div className="clarify-card">
      <div className="clarify-heading">
        <div className="clarify-title-row">
          <div>
            <span className="page-kicker">Before minutes</span>
            <h2>A few things to confirm.</h2>
          </div>
          <button type="button" className="icon-button" onClick={onClose} disabled={busy} aria-label="Close and stop generating" title="Close — stops minutes generation">
            <Icon name="x" size={17} />
          </button>
        </div>
        <p>Bea found points in the transcript that could change the minutes. Answer, type your own, add extra context, or skip — then submit once.</p>
        <div className="clarify-progress" aria-label={`Question ${index + 1} of ${total}`}>
          <span className="clarify-progress-label">Question {index + 1} of {total}{answeredCount > 0 ? ` · ${answeredCount} answered` : ''}</span>
          <div className="clarify-progress-track">{questions.map((_, position) => <span key={position} className={position === index ? 'current' : position < index ? 'done' : ''} />)}</div>
        </div>
      </div>
      <fieldset className="clarify-question">
        <legend>{current.question}</legend>
        {current.options.map((option) => <label key={option} className="clarify-option">
          <input type="radio" name={`q${index}`} value={option} checked={answers[index] === option}
            onChange={() => setAnswer(option)} />
          <span>{option}</span>
        </label>)}
        <input className="clarify-free" placeholder={current.options.length ? 'Or type your own answer…' : 'Type your answer…'} value={answers[index] ?? ''}
          onChange={(event) => setAnswer(event.target.value)} />
        <button type="button" className="clarify-details-toggle" onClick={() => setShowDetails((values) => ({ ...values, [index]: !values[index] }))}>
          <Icon name="plus" size={12} />{showDetails[index] ? 'Hide extra context for Bea' : 'Add extra context for Bea (optional)'}
        </button>
        {showDetails[index] && <textarea className="clarify-details" rows={3} placeholder="Anything else Bea should know about this answer — it is passed to the model when writing the minutes…" value={details[index] ?? ''} onChange={(event) => setDetails((values) => ({ ...values, [index]: event.target.value }))} aria-label="Extra context for this answer" />}
      </fieldset>
      <div className="clarify-actions">
        <button className="secondary" onClick={back} disabled={index === 0}><Icon name="arrow-left" size={14} />Back</button>
        <button className="secondary" onClick={next} disabled={isLast} title="Leave this question unanswered and move on">Skip <Icon name="arrow-right" size={14} /></button>
        {isLast
          ? <button className="primary" onClick={submit} title="Answers are folded into the minutes"><Icon name="check" size={14} />{busy ? 'Queued — generating…' : 'Submit and generate'}</button>
          : <button className="primary" onClick={next} title="Keep this answer and continue">Next <Icon name="arrow-right" size={14} /></button>}
      </div>
    </div>
  </div>;
}
