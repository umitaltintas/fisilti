import i18next from "eslint-plugin-i18next";
import tsParser from "@typescript-eslint/parser";
import tsPlugin from "@typescript-eslint/eslint-plugin";
import reactHooks from "eslint-plugin-react-hooks";
import jsxA11y from "eslint-plugin-jsx-a11y";

export default [
  {
    files: ["src/**/*.{ts,tsx}"],
    languageOptions: {
      parser: tsParser,
      parserOptions: {
        ecmaFeatures: {
          jsx: true,
        },
        // Typed linting, needed by no-floating-promises.
        projectService: true,
        tsconfigRootDir: import.meta.dirname,
      },
    },
    plugins: {
      i18next,
      "@typescript-eslint": tsPlugin,
      "react-hooks": reactHooks,
      "jsx-a11y": jsxA11y,
    },
    rules: {
      // Catch text in JSX that should be translated. User-visible attributes
      // (aria-label, title, placeholder, alt) are checked too; purely technical
      // ones are not.
      "i18next/no-literal-string": [
        "error",
        {
          mode: "jsx-only",
          "jsx-attributes": {
            include: [
              "aria-label",
              "aria-description",
              "title",
              "placeholder",
              "alt",
              "label",
              "description",
            ],
          },
        },
      ],

      ...jsxA11y.flatConfigs.recommended.rules,
      // The settings rows bind labels by id/aria-labelledby, which this rule
      // cannot follow through our wrapper components.
      "jsx-a11y/label-has-associated-control": [
        "error",
        { assert: "either", depth: 3 },
      ],

      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "warn",

      // An un-awaited promise swallows its rejection: the user sees nothing
      // and the console gets an "unhandled rejection". Await it, `void` it on
      // purpose, or attach a handler.
      "@typescript-eslint/no-floating-promises": "error",
      "@typescript-eslint/no-misused-promises": [
        "error",
        { checksVoidReturn: { attributes: false } },
      ],
    },
  },
];
