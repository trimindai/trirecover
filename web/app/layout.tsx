import type { Metadata } from "next";
import "./globals.css";

export const metadata: Metadata = {
  title: "TriRecover — Read-only data recovery for Windows",
  description:
    "Professional, forensically-sound file recovery. The source drive is opened read-only. Built by TriMind AI.",
  metadataBase: new URL("https://trirecover.trimind.tech"),
  openGraph: {
    title: "TriRecover",
    description:
      "Read-only data recovery for Windows. Recover deleted photos, videos, and documents from disk images — without ever writing to the source.",
    url: "https://trirecover.trimind.tech",
    siteName: "TriRecover",
    type: "website",
  },
  twitter: {
    card: "summary_large_image",
    title: "TriRecover",
    description: "Read-only data recovery for Windows.",
  },
};

export default function RootLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return (
    <html lang="en">
      <body className="font-sans antialiased">{children}</body>
    </html>
  );
}
