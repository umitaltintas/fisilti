import React from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";

// Markdown renderer for meeting notes/summaries. The backend produces
// structured markdown (headings, bullets, checkboxes, tables); we render it
// with theme-matched typography instead of plain `whitespace-pre-wrap`.
//
// Components are mapped explicitly so the output uses handy's design tokens
// (text/mid-gray/logo-primary) rather than relying on a prose plugin.
const components: Components = {
  h1: ({ children }) => (
    <h1 className="text-base font-semibold text-text mt-3 mb-1.5 first:mt-0 break-words">
      {children}
    </h1>
  ),
  h2: ({ children }) => (
    <h2 className="text-sm font-semibold text-text mt-3 mb-1.5 first:mt-0 break-words">
      {children}
    </h2>
  ),
  h3: ({ children }) => (
    <h3 className="text-sm font-medium text-text/90 mt-2.5 mb-1 first:mt-0 break-words">
      {children}
    </h3>
  ),
  h4: ({ children }) => (
    <h4 className="text-xs font-semibold uppercase tracking-wide text-mid-gray mt-2.5 mb-1 first:mt-0 break-words">
      {children}
    </h4>
  ),
  p: ({ children }) => (
    <p className="text-sm text-text/90 leading-relaxed my-1.5 break-words">
      {children}
    </p>
  ),
  ul: ({ children }) => (
    <ul className="list-disc pl-5 my-1.5 space-y-1 text-sm text-text/90 marker:text-logo-primary">
      {children}
    </ul>
  ),
  ol: ({ children }) => (
    <ol className="list-decimal pl-5 my-1.5 space-y-1 text-sm text-text/90 marker:text-logo-primary">
      {children}
    </ol>
  ),
  li: ({ children }) => <li className="break-words">{children}</li>,
  input: ({ checked, type }) =>
    type === "checkbox" ? (
      <input
        type="checkbox"
        checked={!!checked}
        readOnly
        className="mr-1.5 align-middle accent-logo-primary"
      />
    ) : null,
  a: ({ children, href }) => (
    <a
      href={href}
      target="_blank"
      rel="noreferrer"
      className="text-logo-primary underline underline-offset-2 hover:opacity-80 break-words"
    >
      {children}
    </a>
  ),
  strong: ({ children }) => (
    <strong className="font-semibold text-text">{children}</strong>
  ),
  em: ({ children }) => <em className="italic">{children}</em>,
  blockquote: ({ children }) => (
    <blockquote className="border-l-2 border-logo-primary/40 pl-3 my-2 text-sm text-text/70 italic">
      {children}
    </blockquote>
  ),
  code: ({ children }) => (
    <code className="rounded bg-mid-gray/15 px-1 py-0.5 font-mono text-[0.8em] text-text/90">
      {children}
    </code>
  ),
  pre: ({ children }) => (
    <pre className="my-2 overflow-x-auto rounded-md bg-mid-gray/10 p-3 text-xs font-mono text-text/90">
      {children}
    </pre>
  ),
  hr: () => <hr className="my-3 border-mid-gray/20" />,
  table: ({ children }) => (
    <div className="my-2 overflow-x-auto">
      <table className="w-full border-collapse text-sm">{children}</table>
    </div>
  ),
  th: ({ children }) => (
    <th className="border border-mid-gray/20 bg-mid-gray/10 px-2 py-1 text-left font-medium text-text">
      {children}
    </th>
  ),
  td: ({ children }) => (
    <td className="border border-mid-gray/20 px-2 py-1 text-text/90 align-top">
      {children}
    </td>
  ),
};

interface MarkdownProps {
  children: string;
  className?: string;
}

/** Render a markdown string with handy-themed typography. */
export const Markdown: React.FC<MarkdownProps> = ({
  children,
  className = "",
}) => (
  <div className={`select-text ${className}`}>
    <ReactMarkdown remarkPlugins={[remarkGfm]} components={components}>
      {children}
    </ReactMarkdown>
  </div>
);

export default Markdown;
