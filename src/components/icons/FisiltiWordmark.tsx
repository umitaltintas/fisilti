// Brand wordmark text. Defined as a constant (not inline JSX text) so the
// i18next no-literal-string lint rule doesn't flag the product name.
const BRAND_NAME = "Fisilti";

const FisiltiWordmark = ({
  width,
  height,
  className,
}: {
  width?: number;
  height?: number;
  className?: string;
}) => {
  return (
    <svg
      width={width}
      height={height}
      className={className}
      viewBox="0 0 360 96"
      fill="none"
      xmlns="http://www.w3.org/2000/svg"
    >
      {/*
        Fisilti wordmark. A small whisper-ripple mark sits before the
        word. The text and mark use `.logo-primary` (fill via the brand
        var --color-logo-primary), so the wordmark rebrands with the theme.
        Note Turkish orthography: dotless "ı" and dotted "i".
      */}
      <g className="logo-primary">
        {/* ripple mark */}
        <circle cx="14" cy="50" r="6" />
      </g>
      <g
        className="logo-primary"
        fill="none"
        stroke="var(--color-logo-primary)"
        strokeLinecap="round"
      >
        <path d="M27 38a18 18 0 0 1 0 24" strokeWidth="5" opacity="0.85" />
        <path d="M38 30a30 30 0 0 1 0 40" strokeWidth="4.5" opacity="0.45" />
      </g>
      <text
        x="60"
        y="50"
        dominantBaseline="central"
        className="logo-primary"
        fontFamily="-apple-system, BlinkMacSystemFont, 'Segoe UI', Inter, system-ui, sans-serif"
        fontSize="52"
        fontWeight="650"
        letterSpacing="-1"
      >
        {BRAND_NAME}
      </text>
    </svg>
  );
};

export default FisiltiWordmark;
