const FisiltiMark = ({
  width,
  height,
  className,
}: {
  width?: number | string;
  height?: number | string;
  className?: string;
}) => (
  <svg
    width={width || 126}
    height={height || 126}
    viewBox="0 0 126 126"
    className={`text-logo-primary ${className ?? ""}`.trim()}
    fill="none"
    xmlns="http://www.w3.org/2000/svg"
  >
    {/*
      Fısıltı mark — a "whisper" glyph: a solid source dot with three
      concentric sound ripples radiating to the right, like a quiet
      voice signal travelling outward. Uses currentColor so it inherits
      the brand purple from `.text-logo-primary` / `text-logo-primary`.
    */}
    <g stroke="currentColor" strokeLinecap="round" fill="none">
      {/* source dot */}
      <circle cx="33" cy="63" r="9" fill="currentColor" stroke="none" />
      {/* inner ripple */}
      <path d="M52 45a30 30 0 0 1 0 36" strokeWidth="9" opacity="0.9" />
      {/* mid ripple */}
      <path d="M68 32a52 52 0 0 1 0 62" strokeWidth="8" opacity="0.6" />
      {/* outer ripple */}
      <path d="M84 21a74 74 0 0 1 0 84" strokeWidth="7" opacity="0.32" />
    </g>
  </svg>
);

export default FisiltiMark;
