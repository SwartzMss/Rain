/** @type {import('tailwindcss').Config} */
module.exports = {
  content: ['./index.html', './src/**/*.{ts,tsx}'],
  theme: {
    extend: {
      colors: {
        brand: {
          500: '#61dafb',
          700: '#3cb7d6'
        }
      }
    }
  },
  plugins: []
};
