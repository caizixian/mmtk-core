/** @type {import('tailwindcss').Config} */
module.exports = {
  content: ['./src/**/*.{html,js,jsx,ts,tsx}'],
  darkMode: 'class',
  theme: {
    extend: {
      fontFamily: {
        sans: ['Inter', 'system-ui', 'sans-serif'],
        mono: ['JetBrains Mono', 'Fira Code', 'monospace'],
      },
      colors: {
        surface: {
          DEFAULT: '#161822',
          card: '#1c1f2e',
          hover: '#252840',
        },
        border: {
          DEFAULT: '#2a2d42',
          focus: '#5b6ef5',
        },
      },
    },
  },
  plugins: [],
}
