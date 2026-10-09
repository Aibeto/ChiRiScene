import { defineConfig } from 'vitepress'

// https://vitepress.dev/reference/site-config
export default defineConfig({
  title: "ChiRi Docs",
  description: "ChiRi Schedule Documents",
  themeConfig: {
    // https://vitepress.dev/reference/default-theme-config
    nav: [
      { text: '首页', link: '/' },
      { text: '快速开始', link: '/quick-start' }
    ],

    sidebar: [
      {
        text: '使用模块',
        items: [
          { text: '快速开始', link: '/quick-start' },
          { text: '日志和遥测', link: '/about-logs' }
        ]
      },
    ],

    socialLinks: [
      { icon: 'github', link: 'https://github.com/Aibeto/ChiRiScene' }
    ]
  }
})
