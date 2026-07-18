/** @type {import('tailwindcss').Config} */
module.exports = {
  content: ['./index.html', './src/**/*.{ts,tsx}'],
  theme: {
    extend: {
      colors: {
        slate: {
          50: '#f4f7fb',
          100: '#e8eef6',
          200: '#d3deeb',
          300: '#b2c2d4',
          400: '#8296ad',
          500: '#5f748c',
          600: '#43586f',
          700: '#30445a',
          800: '#1d3044',
          900: '#112235',
          950: '#071522'
        },
        sky: {
          50: '#ecfeff',
          100: '#cffafe',
          200: '#a5f3fc',
          300: '#67e8f9',
          400: '#22d3ee',
          500: '#06b6d4',
          600: '#0891b2',
          700: '#0e7490',
          800: '#155e75',
          900: '#164e63'
        },
        brand: {
          500: '#14b8a6',
          700: '#0f766e'
        }
      }
    }
  },
  plugins: []
};
