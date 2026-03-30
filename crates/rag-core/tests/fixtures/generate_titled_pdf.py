#!/usr/bin/env python3
"""Generate a PDF with /Title and /Author metadata for testing.

Run once to regenerate the committed titled.pdf fixture:

    uv add 'fpdf2==2.8.2'
    python generate_titled_pdf.py

The output is a binary fixture committed to the repo. The script exists
solely for reproducibility — it is NOT executed during tests or CI.
"""
from fpdf import FPDF

pdf = FPDF()
pdf.set_title("Apex Test Document")
pdf.set_author("Test Author")
pdf.add_page()
pdf.set_font("Helvetica", size=12)
pdf.cell(text="This PDF has title and author metadata.")
pdf.output("titled.pdf")
print("Generated titled.pdf")
