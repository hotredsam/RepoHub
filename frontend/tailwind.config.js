/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  darkMode: "class",
  theme: {
    extend: {
      colors: {
        ink: "#0b0e14",
        panel: "#11151f",
        edge: "#1e2633",
        accent: "#5b9dff",
      },
    },
  },
  plugins: [],
};
