declare const languages: unknown[];
declare const parsers: Record<string, unknown>;
declare const printers: Record<string, unknown>;
declare const options: {
  zhfmtWhitelist: {
    category: string;
    type: string;
    array: true;
    default: [{ value: string[] }];
    description: string;
  };
};

export { languages, parsers, printers, options };
