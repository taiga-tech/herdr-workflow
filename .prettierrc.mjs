/** @type {import("prettier").Config} */
const prettierConfig = {
    semi: false,
    singleQuote: true,
    printWidth: 80,
    tabWidth: 2,
    trailingComma: 'es5',
    bracketSameLine: false,
    singleAttributePerLine: false,
    overrides: [
        { files: '**/*.md', options: { tabWidth: 4 } },
        { files: '**/*.mdx', options: { tabWidth: 4 } },
        { files: '**/*.html', options: { tabWidth: 4 } },
        { files: '**/*.js', options: { tabWidth: 4 } },
        { files: '**/*.mjs', options: { tabWidth: 4 } },
        { files: '**/*.jsx', options: { tabWidth: 4 } },
        { files: '**/*.ts', options: { tabWidth: 4 } },
        { files: '**/*.tsx', options: { tabWidth: 4 } },
        { files: '**/*.json', options: { tabWidth: 4 } },
        { files: '**/*.xml', options: { tabWidth: 4 } },
    ],
}

export default prettierConfig
