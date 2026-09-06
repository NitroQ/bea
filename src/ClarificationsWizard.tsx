import { useState } from 'react';
import type { ClarificationQuestion } from './chatActions';
import Icon from './Icon';

export type ClarificationAnswer = { question: string; answer: string };
type Props = {
  questions: ClarificationQuestion[];
  busy: boolean;
  onAnswered: (answers: ClarificationAnswer[]) => void;
  onSkip: () => void;
};

/// One screen per question: options as radio buttons plus a free-text field.
/// The free-text field is always available so the user can go off-menu.
export default function ClarificationsWizard({ questions, busy, onAnswered, onSkip }: Props) {
  const [answers, setAnswers] = useState<Record<number, string>>({});
  const current = 0; // all questions on one scrollable card, simplest v1
  void current;
  return <div className="clarify-wizard">
    <div className="clarify-heading">
      <span className="page-kicker">Before minutes</span>
      <h2>A few things to confirm.</h2>
      <p>Bea found points in the transcript that could change the minutes. Answer, type your own, or skip any question.</p>
    </div>
    {questions.map((item, index) => <fieldset key={index} className="clarify-question">
      <legend>{item.question}</legend>
      {item.options.map((option) => <label key={option} className="clarify-option">
        <input type="radio" name={`q${index}`} value={option} checked={answers[index] === option}
          onChange={() => setAnswers((values) => ({ ...values, [index]: option }))} />
        <span>{option}</span>
      </label>)}
      <input className="clarify-free" placeholder="Or type your own answer…" value={answers[index] ?? ''}
        onChange={(event) => setAnswers((values) => ({ ...values, [index]: event.target.value }))} />
    </fieldset>)}
    <div className="clarify-actions">
      <button className="secondary" onClick={onSkip} disabled={busy}>Skip — generate now</button>
      <button className="primary" disabled={busy} onClick={() => onAnswered(questions.map((item, index) => ({ question: item.question, answer: answers[index] ?? '' })).filter((pair) => pair.answer.trim()))}>
        <Icon name="check" size={14} />Confirm and generate
      </button>
    </div>
  </div>;
}
